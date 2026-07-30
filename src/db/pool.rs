use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Executor, PgPool, Postgres, Sqlite, SqlitePool};
use tokio::sync::Notify;
use tracing::info;

use crate::config::Backend;

/// Embedded migrations, one set per backend. The two dialects diverge enough
/// (identity columns, timestamp types, `WITHOUT ROWID`, expression-index
/// syntax) that a single migration file cannot serve both; the correct set is
/// selected at connect time by [`Backend`].
static SQLITE_MIGRATOR: Migrator = sqlx::migrate!("migrations/sqlite");
static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

/// The backend-tagged `sqlx` pool held inside a [`Db`]. Chosen once from
/// `DATABASE_URL` (see [`Backend`]) and never changed for the process lifetime.
/// Cloning is cheap — `sqlx` pools are `Arc`-backed handles.
#[derive(Clone)]
pub enum DbInner {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

/// Scheduling priority of a [`Db`] handle. Every handle carries one; the default
/// `User` handle lives in `AppState`, and background workers derive a
/// `Background` handle via [`Db::background`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Priority {
    /// Interactive, user-facing work (handlers, middleware).
    User,
    /// Background work (feed sync, summary worker, retention, backfill).
    Background,
}

/// `SQLite` write-priority scheduler. `SQLite` serializes writers, so a background
/// batch (a feed-sync entry upsert, a retention delete run) can make an
/// interactive click wait on the single write lock. This restores the priority
/// the pre-sqlx actor gave: background DB operations yield at their boundary
/// while any interactive operation is in flight.
///
/// It is a thin admission gate, not a queue: `User` ops increment `inflight`
/// for their duration; `Background` ops await `inflight == 0` before running.
/// `PostgreSQL` has real writer concurrency (MVCC), so its handles never touch
/// this — the gate is a no-op there.
#[derive(Default)]
struct SqliteSched {
    /// Count of in-flight `User`-priority operations.
    inflight: AtomicUsize,
    /// Notified when `inflight` drops to zero, waking waiting background ops.
    idle: Notify,
}

impl SqliteSched {
    /// Register a `User` operation and return a guard that unregisters it (and
    /// wakes background waiters when the last one finishes) on drop.
    fn enter_user(self: &Arc<Self>) -> UserGuard {
        self.inflight.fetch_add(1, Ordering::AcqRel);
        UserGuard(self.clone())
    }

    /// Block until no `User` operation is in flight. Called by background ops
    /// before they touch the write lock so interactive work goes first.
    async fn wait_for_idle(&self) {
        loop {
            // Register for the wakeup *before* the final check so a
            // notify_waiters() between the check and the await can't be lost.
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inflight.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}

/// RAII marker for an in-flight `User` operation (see [`SqliteSched`]).
pub struct UserGuard(Arc<SqliteSched>);

impl Drop for UserGuard {
    fn drop(&mut self) {
        if self.0.inflight.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Last user op finished — let background work proceed.
            self.0.idle.notify_waiters();
        }
    }
}

/// A boot-time-selected database handle: a backend pool plus a scheduling
/// [`Priority`] and a shared `SQLite` write scheduler. Every query dispatches on
/// the inner two-armed enum via the `query_*!` macros, which consult the
/// priority/scheduler so background work yields to interactive work on `SQLite`.
///
/// Cloning is cheap (pool + `Arc` handles) and preserves the priority; use
/// [`Db::background`] to derive a background-priority handle sharing the same
/// pool and scheduler.
#[derive(Clone)]
pub struct Db {
    inner: DbInner,
    sched: Arc<SqliteSched>,
    priority: Priority,
}

/// A backend-tagged transaction, the unit-of-work boundary for operations that
/// compose several model calls atomically (OPML import, entry upserts, `GReader`
/// tag edits). Inner model calls execute against `&mut Tx` via the `*_tx!`
/// macros. Obtained from [`Db::begin`]; finished with [`Tx::commit`] or
/// [`Tx::rollback`] (a dropped `Tx` rolls back).
///
/// The optional `_guard` on the `SQLite` variant holds the write-priority
/// admission for the whole transaction: acquired at [`Db::begin`] (a background
/// tx waits for interactive idle first) and released when the tx is dropped.
pub enum Tx<'c> {
    Sqlite {
        tx: sqlx::Transaction<'c, Sqlite>,
        _guard: Option<UserGuard>,
    },
    Postgres(sqlx::Transaction<'c, Postgres>),
}

impl Db {
    /// Open the pool for `url` under the given `backend` and run that backend's
    /// migrations. For `SQLite`, `url` is a filesystem path (WAL mode, tuning
    /// pragmas, create-if-missing); for `PostgreSQL` it is a `postgres://` URL.
    pub async fn connect(url: &str, backend: Backend) -> Result<Self, sqlx::Error> {
        let db = match backend {
            Backend::Sqlite => {
                // `url` is a bare file path here (e.g. "rdrs.sqlite3"), not a
                // sqlite: URL, so build options from the filename directly.
                // Tuning mirrors the pre-sqlx actor: WAL + synchronous=NORMAL is
                // durable-to-checkpoint and skips a per-commit fsync; the cache /
                // mmap / temp_store / busy_timeout pragmas match the old values.
                let opts = SqliteConnectOptions::from_str(&format!("sqlite://{url}"))
                    .unwrap_or_else(|_| SqliteConnectOptions::new().filename(url))
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .synchronous(SqliteSynchronous::Normal)
                    .busy_timeout(Duration::from_secs(5))
                    .pragma("cache_size", "-20000")
                    .pragma("mmap_size", "134217728")
                    .pragma("temp_store", "MEMORY");
                let pool = SqlitePoolOptions::new()
                    .max_connections(5)
                    .connect_with(opts)
                    .await?;
                DbInner::Sqlite(pool)
            }
            Backend::Postgres => {
                // Pin every pooled connection to UTC. Entry timestamps are
                // written with `now()` / naive `%Y-%m-%d %H:%M:%S` string binds
                // and the composite pagination cursor compares them as strings
                // via `to_char(col, 'YYYY-MM-DD HH24:MI:SS')`. All three are
                // interpreted in the session `TimeZone`, so pinning it to UTC
                // makes the PG cursor strings byte-identical to SQLite's
                // `datetime('now')` TEXT — a divergent server default would
                // otherwise shift the cursor and corrupt pagination order.
                let opts = PgConnectOptions::from_str(url)?;
                let pool = PgPoolOptions::new()
                    .after_connect(|conn, _meta| {
                        Box::pin(async move {
                            conn.execute("SET TIME ZONE 'UTC'").await?;
                            Ok(())
                        })
                    })
                    .connect_with(opts)
                    .await?;
                DbInner::Postgres(pool)
            }
        };
        let db = Db {
            inner: db,
            sched: Arc::new(SqliteSched::default()),
            priority: Priority::User,
        };
        db.migrate().await?;
        Ok(db)
    }

    /// Build an in-memory `SQLite` `Db` backed by a single shared connection and
    /// run migrations. Used by the test suites (and available for ephemeral
    /// embedded use): a one-connection pool keeps every query on the same
    /// `:memory:` database instead of spawning a fresh empty one per connection.
    pub async fn connect_in_memory() -> Result<Self, sqlx::Error> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        let db = Db {
            inner: DbInner::Sqlite(pool),
            sched: Arc::new(SqliteSched::default()),
            priority: Priority::User,
        };
        db.migrate().await?;
        Ok(db)
    }

    /// The backend pool this handle dispatches to. Used by the `query_*!` macros
    /// and the dynamic-query helpers to match on the concrete backend.
    pub fn inner(&self) -> &DbInner {
        &self.inner
    }

    /// `true` if this handle is backed by `PostgreSQL` (drives dialect forks).
    pub fn is_postgres(&self) -> bool {
        matches!(self.inner, DbInner::Postgres(_))
    }

    /// Derive a background-priority handle sharing this handle's pool and `SQLite`
    /// scheduler. Background workers (feed sync, summary worker, retention,
    /// backfill) call this once so their DB operations yield to interactive work
    /// on `SQLite`. No-op effect on `PostgreSQL`.
    pub fn background(&self) -> Db {
        Db {
            inner: self.inner.clone(),
            sched: self.sched.clone(),
            priority: Priority::Background,
        }
    }

    /// Acquire this handle's write-priority admission for one operation. On
    /// `SQLite` a `User` op returns a guard tracking it as in-flight; a
    /// `Background` op first waits for interactive idle. On `PostgreSQL` (real
    /// writer concurrency) this is a no-op. The `query_*!` macros hold the
    /// returned guard across the query; [`Db::begin`] holds it across the tx.
    pub async fn admit(&self) -> Option<UserGuard> {
        if matches!(self.inner, DbInner::Sqlite(_)) {
            match self.priority {
                Priority::User => return Some(self.sched.enter_user()),
                Priority::Background => self.sched.wait_for_idle().await,
            }
        }
        None
    }

    /// Run the backend's embedded migrations. Migrations use `IF NOT EXISTS`,
    /// so an existing (pre-sqlx) `SQLite` database is baselined harmlessly: the
    /// consolidated `0001` no-ops against already-present tables and is recorded
    /// in `_sqlx_migrations`.
    async fn migrate(&self) -> Result<(), sqlx::Error> {
        match &self.inner {
            DbInner::Sqlite(pool) => SQLITE_MIGRATOR.run(pool).await,
            DbInner::Postgres(pool) => POSTGRES_MIGRATOR.run(pool).await,
        }
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))
    }

    /// Begin a transaction on the underlying pool. The write-priority admission
    /// is held for the whole transaction (a background tx waits for interactive
    /// idle before starting).
    pub async fn begin(&self) -> Result<Tx<'_>, sqlx::Error> {
        let guard = self.admit().await;
        Ok(match &self.inner {
            DbInner::Sqlite(pool) => Tx::Sqlite {
                // BEGIN IMMEDIATE takes the write lock up front, so a second
                // writer blocks here — where the connection's `busy_timeout`
                // applies and makes it wait — instead of the default DEFERRED
                // behaviour, which starts as a reader and only tries to promote
                // to a writer at the first write. That promotion, when another
                // writer already holds the lock, returns SQLITE_BUSY ("database
                // is locked") *immediately*: SQLite skips the busy handler there
                // to avoid a deadlock, so the 5 s timeout cannot paper over it.
                // Every `begin()` here is a write unit-of-work (upserts, OPML
                // import, GReader tag edits), and the write-priority gate only
                // orders user-vs-background — not the up-to-4 concurrent
                // background feed syncs that race here — so DEFERRED let those
                // collide. Read-only work uses the `query_*!` macros, never
                // `begin()`, so nothing pays for an unnecessary write lock.
                tx: pool.begin_with("BEGIN IMMEDIATE").await?,
                _guard: guard,
            },
            DbInner::Postgres(pool) => Tx::Postgres(pool.begin().await?),
        })
    }

    /// Flush and close the pool. For `SQLite` this truncates the WAL first so no
    /// `-wal`/`-shm` sidecars linger after shutdown.
    pub async fn shutdown(&self) {
        if let DbInner::Sqlite(pool) = &self.inner {
            info!(
                event = "db.checkpoint_started",
                "executing WAL checkpoint before shutdown"
            );
            if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE);")
                .execute(pool)
                .await
            {
                tracing::error!(event = "db.checkpoint_failed", error = %e, "WAL checkpoint failed");
            }
        }
        match &self.inner {
            DbInner::Sqlite(pool) => pool.close().await,
            DbInner::Postgres(pool) => pool.close().await,
        }
    }
}

impl Tx<'_> {
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        match self {
            Tx::Sqlite { tx, .. } => tx.commit().await,
            Tx::Postgres(t) => t.commit().await,
        }
    }

    /// Roll the transaction back explicitly (dropping also rolls back).
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        match self {
            Tx::Sqlite { tx, .. } => tx.rollback().await,
            Tx::Postgres(t) => t.rollback().await,
        }
    }
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner {
            DbInner::Sqlite(_) => write!(f, "Db::Sqlite({:?})", self.priority),
            DbInner::Postgres(_) => write!(f, "Db::Postgres({:?})", self.priority),
        }
    }
}

/// `true` if `e` is a UNIQUE / primary-key constraint violation, on either
/// backend. Model layers use this to translate a duplicate insert into a domain
/// error (e.g. `AppError::CategoryExists`).
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.kind() == sqlx::error::ErrorKind::UniqueViolation)
}

/// Rewrite the `SQLite`-dialect fragments in a macro `$sql` literal to their
/// `PostgreSQL` equivalents at dispatch time. Applied *only* in the Postgres arm
/// of the `query_*!` / `db_execute!` macros so a model writes one SQL literal
/// that runs correctly on both backends.
///
/// Currently rewrites the `SQLite` scalar `datetime('now')` — which yields the
/// `%Y-%m-%d %H:%M:%S` TEXT that the composite pagination cursor and the
/// timestamp-column DEFAULTs depend on — to `PostgreSQL` `now()` (a `timestamptz`
/// that, under the connection's pinned `TimeZone=UTC`, encodes to the same
/// instant). The exact literal token is matched, so the comma-modifier forms
/// (`datetime('now', '-25 hours')`, `datetime('now', $2)`) are deliberately
/// left untouched — those need interval arithmetic and are handled with
/// explicit `Dialect` forks at their call sites.
#[doc(hidden)]
pub fn pg_rewrite(sql: &str) -> String {
    sql.replace("datetime('now')", "now()")
}

// --- dispatch macros -------------------------------------------------------
//
// These collapse the two-arm `match db { Sqlite(..) => .., Postgres(..) => .. }`
// so a model function writes its SQL and binds exactly once. The same `$sql`
// literal and `$bind` list are used for both backends; placeholders are `$N`
// (a SQLite superset PostgreSQL requires) and `RETURNING` is used in place of
// `last_insert_rowid()`. Bind arguments are evaluated in *both* arms, so pass
// `Copy` values or references (`&str`, `i64`, `DateTime<Utc>`, `&[u8]`).
//
// The Postgres arm runs `$sql` through `pg_rewrite` (see above) so SQLite-only
// scalars like `datetime('now')` become their PG equivalents; the SQLite arm
// uses the literal verbatim to keep its prepared-statement cache keyed on it.
//
// `$ty` must derive `sqlx::FromRow` (its generated impl is row-generic, so one
// derive serves both `SqliteRow` and `PgRow`). Each macro has a `_tx` sibling
// that runs against `&mut Tx` for transactional composition.

// Each non-tx macro binds `$db` once, takes the write-priority admission
// (`admit()` — a User op registers as in-flight; a Background op waits for SQLite
// interactive idle; no-op on PG), runs the query while holding it, then releases.

/// `SELECT` exactly one row as `$ty`.
#[macro_export]
macro_rules! query_one {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __db = $db;
        let __guard = __db.admit().await;
        let __r = match __db.inner() {
            $crate::db::DbInner::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_one(pool).await
            }
            $crate::db::DbInner::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_one(pool).await
            }
        };
        ::core::mem::drop(__guard);
        __r
    }};
}

/// `SELECT` zero or one row as `Option<$ty>`.
#[macro_export]
macro_rules! query_opt {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __db = $db;
        let __guard = __db.admit().await;
        let __r = match __db.inner() {
            $crate::db::DbInner::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_optional(pool).await
            }
            $crate::db::DbInner::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_optional(pool).await
            }
        };
        ::core::mem::drop(__guard);
        __r
    }};
}

/// `SELECT` many rows as `Vec<$ty>`.
#[macro_export]
macro_rules! query_all {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __db = $db;
        let __guard = __db.admit().await;
        let __r = match __db.inner() {
            $crate::db::DbInner::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_all(pool).await
            }
            $crate::db::DbInner::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_all(pool).await
            }
        };
        ::core::mem::drop(__guard);
        __r
    }};
}

/// `SELECT` a single scalar column as `$ty` (e.g. `COUNT(*)` as `i64`).
#[macro_export]
macro_rules! query_scalar {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __db = $db;
        let __guard = __db.admit().await;
        let __r = match __db.inner() {
            $crate::db::DbInner::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_scalar::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_one(pool).await
            }
            $crate::db::DbInner::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_scalar::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_one(pool).await
            }
        };
        ::core::mem::drop(__guard);
        __r
    }};
}

/// Run a statement (INSERT/UPDATE/DELETE) and return rows affected as `u64`.
#[macro_export]
macro_rules! db_execute {
    ($db:expr, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __db = $db;
        let __guard = __db.admit().await;
        let __r = match __db.inner() {
            $crate::db::DbInner::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query::<::sqlx::Sqlite>($sql);
                $( q = q.bind($bind); )*
                q.execute(pool).await.map(|r| r.rows_affected())
            }
            $crate::db::DbInner::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query::<::sqlx::Postgres>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.execute(pool).await.map(|r| r.rows_affected())
            }
        };
        ::core::mem::drop(__guard);
        __r
    }};
}

/// `query_one!` against `&mut Tx`.
#[macro_export]
macro_rules! query_one_tx {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $tx {
            $crate::db::Tx::Sqlite { tx: t, .. } => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_one(&mut **t).await
            }
            $crate::db::Tx::Postgres(t) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_one(&mut **t).await
            }
        }
    }};
}

/// `query_opt!` against `&mut Tx`.
#[macro_export]
macro_rules! query_opt_tx {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $tx {
            $crate::db::Tx::Sqlite { tx: t, .. } => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_optional(&mut **t).await
            }
            $crate::db::Tx::Postgres(t) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_optional(&mut **t).await
            }
        }
    }};
}

/// `query_all!` against `&mut Tx`.
#[macro_export]
macro_rules! query_all_tx {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $tx {
            $crate::db::Tx::Sqlite { tx: t, .. } => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_all(&mut **t).await
            }
            $crate::db::Tx::Postgres(t) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_all(&mut **t).await
            }
        }
    }};
}

/// `query_scalar!` against `&mut Tx`.
#[macro_export]
macro_rules! query_scalar_tx {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $tx {
            $crate::db::Tx::Sqlite { tx: t, .. } => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_scalar::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_one(&mut **t).await
            }
            $crate::db::Tx::Postgres(t) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_scalar::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_one(&mut **t).await
            }
        }
    }};
}

/// `db_execute!` against `&mut Tx`.
#[macro_export]
macro_rules! db_execute_tx {
    ($tx:expr, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $tx {
            $crate::db::Tx::Sqlite { tx: t, .. } => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query::<::sqlx::Sqlite>($sql);
                $( q = q.bind($bind); )*
                q.execute(&mut **t).await.map(|r| r.rows_affected())
            }
            $crate::db::Tx::Postgres(t) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query::<::sqlx::Postgres>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.execute(&mut **t).await.map(|r| r.rows_affected())
            }
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    // The write-priority guarantee: a background op must not proceed past its
    // admission while any user op is in flight. Asserted via ordering — the
    // background task checks a flag that the test sets only after the user op is
    // done, and can only observe it once the gate lets it through.
    #[tokio::test]
    async fn sched_background_yields_until_user_finishes() {
        let sched = Arc::new(SqliteSched::default());
        let user_done = Arc::new(AtomicBool::new(false));

        let user = sched.enter_user();
        let bg = {
            let sched = sched.clone();
            let user_done = user_done.clone();
            tokio::spawn(async move {
                sched.wait_for_idle().await;
                assert!(
                    user_done.load(Ordering::Acquire),
                    "background proceeded before the user op finished"
                );
            })
        };

        // Let the background task reach (and park on) its wait.
        tokio::task::yield_now().await;
        // Publish "user finished" before releasing the gate; the background task
        // is still parked (inflight > 0), so it can only wake after this.
        user_done.store(true, Ordering::Release);
        drop(user);

        tokio::time::timeout(Duration::from_secs(1), bg)
            .await
            .expect("background should proceed once the user op finishes")
            .unwrap();
    }

    #[tokio::test]
    async fn sched_idle_lets_background_through_immediately() {
        let sched = Arc::new(SqliteSched::default());
        // No user op in flight — must not block.
        tokio::time::timeout(Duration::from_secs(1), sched.wait_for_idle())
            .await
            .expect("wait_for_idle must return immediately when idle");
    }

    #[tokio::test]
    async fn sched_waits_for_all_users() {
        let sched = Arc::new(SqliteSched::default());
        let both_done = Arc::new(AtomicBool::new(false));

        let g1 = sched.enter_user();
        let g2 = sched.enter_user();
        let bg = {
            let sched = sched.clone();
            let both_done = both_done.clone();
            tokio::spawn(async move {
                sched.wait_for_idle().await;
                assert!(both_done.load(Ordering::Acquire));
            })
        };

        tokio::task::yield_now().await;
        drop(g1); // one still in flight → background stays gated
        tokio::task::yield_now().await;
        both_done.store(true, Ordering::Release);
        drop(g2);

        tokio::time::timeout(Duration::from_secs(1), bg)
            .await
            .expect("background proceeds only after the last user op")
            .unwrap();
    }

    // The full Db path: a background handle's `admit()` gates behind a user
    // handle's in-flight op on SQLite.
    #[tokio::test]
    async fn admit_gates_background_behind_user_on_sqlite() {
        let db = Db::connect_in_memory().await.unwrap();
        let bg = db.background();
        let user_done = Arc::new(AtomicBool::new(false));

        let user_guard = db.admit().await;
        assert!(
            user_guard.is_some(),
            "user op on SQLite registers in-flight"
        );

        let task = {
            let bg = bg.clone();
            let user_done = user_done.clone();
            tokio::spawn(async move {
                let _g = bg.admit().await; // background: waits for idle
                assert!(user_done.load(Ordering::Acquire));
            })
        };

        tokio::task::yield_now().await;
        user_done.store(true, Ordering::Release);
        drop(user_guard);

        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("background admit proceeds after the user op finishes")
            .unwrap();
    }

    // Regression: a full bucket runs up to 4 background feed syncs at once, each
    // opening a write transaction. Under the default `BEGIN DEFERRED` their
    // read→write promotions race and SQLite returns SQLITE_BUSY ("database is
    // locked") that `busy_timeout` cannot retry; `begin()` uses `BEGIN
    // IMMEDIATE` so they queue on the write lock instead. This needs a *file*
    // database — the in-memory pool is a single shared connection and so cannot
    // exhibit the multi-connection contention this guards against.
    #[tokio::test]
    async fn concurrent_write_transactions_do_not_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite3");
        let db = Db::connect(path.to_str().unwrap(), Backend::Sqlite)
            .await
            .unwrap();

        if let DbInner::Sqlite(pool) = &db.inner {
            sqlx::query("CREATE TABLE probe (id INTEGER PRIMARY KEY, n INTEGER)")
                .execute(pool)
                .await
                .unwrap();
        }

        // Fire many background write transactions concurrently — more than the
        // pool's connection count, so they genuinely contend for the writer.
        let mut set = tokio::task::JoinSet::new();
        for i in 0..32_i64 {
            let bg = db.background();
            set.spawn(async move {
                let mut tx = bg.begin().await?;
                if let Tx::Sqlite { tx: sqtx, .. } = &mut tx {
                    // Read *then* write inside the transaction — the pattern a
                    // feed sync uses. Under DEFERRED the SELECT takes a read
                    // snapshot and the INSERT then races to promote to a writer,
                    // which is what surfaces the lock; IMMEDIATE already holds
                    // the write lock so there is no promotion to lose.
                    let _n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe")
                        .fetch_one(&mut **sqtx)
                        .await?;
                    sqlx::query("INSERT INTO probe (n) VALUES (?)")
                        .bind(i)
                        .execute(&mut **sqtx)
                        .await?;
                }
                tx.commit().await
            });
        }

        while let Some(joined) = set.join_next().await {
            joined
                .expect("task panicked")
                .expect("a concurrent write transaction must not fail with a lock");
        }

        if let DbInner::Sqlite(pool) = &db.inner {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe")
                .fetch_one(pool)
                .await
                .unwrap();
            assert_eq!(
                count, 32,
                "every concurrent transaction must have committed"
            );
        }
    }
}
