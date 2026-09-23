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

/// Embedded migrations, one set per backend (the dialects diverge too much to
/// share); selected at connect time by [`Backend`].
static SQLITE_MIGRATOR: Migrator = sqlx::migrate!("migrations/sqlite");
static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

/// The backend-tagged `sqlx` pool inside a [`Db`], fixed for the process lifetime.
#[derive(Clone)]
pub enum DbInner {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

/// Scheduling priority of a [`Db`] handle; background workers derive theirs via
/// [`Db::background`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Priority {
    User,
    /// Feed sync, summary worker, retention, backfill.
    Background,
}

/// `SQLite` write-priority admission gate (not a queue): `User` ops count as
/// in-flight, `Background` ops wait for zero, so a background batch never makes
/// an interactive click wait on the single writer. No-op on `PostgreSQL`.
#[derive(Default)]
struct SqliteSched {
    inflight: AtomicUsize,
    /// Notified when `inflight` drops to zero.
    idle: Notify,
}

impl SqliteSched {
    /// Register a `User` operation; the guard unregisters it on drop.
    fn enter_user(self: &Arc<Self>) -> UserGuard {
        self.inflight.fetch_add(1, Ordering::AcqRel);
        UserGuard(self.clone())
    }

    /// Wait until no `User` operation is in flight.
    async fn wait_for_idle(&self) {
        loop {
            // Register before the check so a notify in between isn't lost.
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

/// RAII marker for an in-flight `User` operation (see `SqliteSched`).
pub struct UserGuard(Arc<SqliteSched>);

impl Drop for UserGuard {
    fn drop(&mut self) {
        if self.0.inflight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

/// Database handle: a backend pool, a [`Priority`], and the shared `SQLite`
/// write scheduler consulted by the `query_*!` macros. Clones keep the priority.
#[derive(Clone)]
pub struct Db {
    inner: DbInner,
    sched: Arc<SqliteSched>,
    priority: Priority,
}

/// A backend-tagged transaction used via the `*_tx!` macros; a dropped `Tx`
/// rolls back. The `SQLite` `_guard` holds write-priority admission throughout.
pub enum Tx<'c> {
    Sqlite {
        tx: sqlx::Transaction<'c, Sqlite>,
        _guard: Option<UserGuard>,
    },
    Postgres(sqlx::Transaction<'c, Postgres>),
}

impl Db {
    /// Open the pool and run migrations. `url` is a file path for `SQLite`, a
    /// `postgres://` URL for `PostgreSQL`.
    pub async fn connect(url: &str, backend: Backend) -> Result<Self, sqlx::Error> {
        let db = match backend {
            Backend::Sqlite => {
                // WAL + synchronous=NORMAL: durable to checkpoint, no per-commit fsync.
                let opts = SqliteConnectOptions::from_str(&format!("sqlite://{url}"))
                    .unwrap_or_else(|_| SqliteConnectOptions::new().filename(url))
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .synchronous(SqliteSynchronous::Normal)
                    .busy_timeout(Duration::from_secs(5))
                    .pragma("cache_size", "-20000")
                    .pragma("mmap_size", "134217728")
                    .pragma("temp_store", "MEMORY")
                    // Bounds `PRAGMA optimize` sampling so ANALYZE doesn't read
                    // every index in full; 400 is SQLite's recommended value.
                    .pragma("analysis_limit", "400");
                let pool = SqlitePoolOptions::new()
                    .max_connections(5)
                    .connect_with(opts)
                    .await?;
                DbInner::Sqlite(pool)
            }
            Backend::Postgres => {
                // Pin sessions to UTC: timestamps and the `to_char` cursor use the
                // session TimeZone, and any other zone would corrupt pagination.
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
        // Stale or missing planner stats (e.g. new migration indexes) cause bad
        // plans, and retention may never refresh them; a no-op costs microseconds.
        db.optimize().await?;
        Ok(db)
    }

    /// In-memory `SQLite` `Db` with migrations applied. One connection, so every
    /// query hits the same `:memory:` database.
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

    /// The backend pool this handle dispatches to.
    pub fn inner(&self) -> &DbInner {
        &self.inner
    }

    /// `true` if this handle is backed by `PostgreSQL` (drives dialect forks).
    pub fn is_postgres(&self) -> bool {
        matches!(self.inner, DbInner::Postgres(_))
    }

    /// Background-priority handle over the same pool and scheduler; its
    /// operations yield to interactive work on `SQLite`.
    pub fn background(&self) -> Db {
        Db {
            inner: self.inner.clone(),
            sched: self.sched.clone(),
            priority: Priority::Background,
        }
    }

    /// Acquire write-priority admission for one operation (see `SqliteSched`).
    /// Returns a guard only for `User` ops on `SQLite`.
    pub async fn admit(&self) -> Option<UserGuard> {
        if matches!(self.inner, DbInner::Sqlite(_)) {
            match self.priority {
                Priority::User => return Some(self.sched.enter_user()),
                Priority::Background => self.sched.wait_for_idle().await,
            }
        }
        None
    }

    /// Refresh stale `SQLite` planner statistics via `PRAGMA optimize`; no-op on
    /// `PostgreSQL` (autoanalyze).
    pub async fn optimize(&self) -> Result<(), sqlx::Error> {
        if let DbInner::Sqlite(pool) = &self.inner {
            sqlx::query("PRAGMA optimize;").execute(pool).await?;
        }
        Ok(())
    }

    /// Run embedded migrations (`IF NOT EXISTS`, so pre-sqlx databases baseline).
    async fn migrate(&self) -> Result<(), sqlx::Error> {
        match &self.inner {
            DbInner::Sqlite(pool) => SQLITE_MIGRATOR.run(pool).await,
            DbInner::Postgres(pool) => POSTGRES_MIGRATOR.run(pool).await,
        }
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))
    }

    /// Begin a write transaction, holding write-priority admission throughout.
    pub async fn begin(&self) -> Result<Tx<'_>, sqlx::Error> {
        let guard = self.admit().await;
        Ok(match &self.inner {
            DbInner::Sqlite(pool) => Tx::Sqlite {
                // IMMEDIATE, not DEFERRED: a deferred read->write promotion
                // returns SQLITE_BUSY without honoring `busy_timeout`, which
                // concurrent feed syncs hit. Taking the lock up front queues them.
                tx: pool.begin_with("BEGIN IMMEDIATE").await?,
                _guard: guard,
            },
            DbInner::Postgres(pool) => Tx::Postgres(pool.begin().await?),
        })
    }

    /// Close the pool; on `SQLite`, truncate the WAL first so no sidecars linger.
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

/// `true` if `e` is a UNIQUE / primary-key violation on either backend.
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.kind() == sqlx::error::ErrorKind::UniqueViolation)
}

/// Rewrite `SQLite`-only SQL for the Postgres arm: `datetime('now')` -> `now()`
/// (same instant under the pinned UTC zone). Modifier forms are left alone and
/// need explicit `Dialect` forks.
#[doc(hidden)]
pub fn pg_rewrite(sql: &str) -> String {
    sql.replace("datetime('now')", "now()")
}

// --- dispatch macros -------------------------------------------------------
//
// One `$sql` literal and bind list serve both backends (`$N` placeholders,
// `RETURNING`). Binds are evaluated in both arms, so pass `Copy` values or
// references. Only the Postgres arm goes through `pg_rewrite`. Non-tx macros
// hold the `admit()` guard for the query.

#[doc(hidden)]
#[macro_export]
macro_rules! __db_dispatch {
    (db $db:expr; $($rest:tt)*) => {{
        let __db = $db;
        let __guard = __db.admit().await;
        let __r = match __db.inner() {
            $crate::db::DbInner::Sqlite(pool) => $crate::__db_dispatch!(@sqlite pool; $($rest)*),
            $crate::db::DbInner::Postgres(pool) => $crate::__db_dispatch!(@pg pool; $($rest)*),
        };
        ::core::mem::drop(__guard);
        __r
    }};
    (tx $tx:expr; $($rest:tt)*) => {
        match $tx {
            $crate::db::Tx::Sqlite { tx: t, .. } => $crate::__db_dispatch!(@sqlite &mut **t; $($rest)*),
            $crate::db::Tx::Postgres(t) => $crate::__db_dispatch!(@pg &mut **t; $($rest)*),
        }
    };
    (@sqlite $exec:expr; $ctor:ident [$($ty:ty)?] $method:ident $(=> $map:expr)?; $sql:expr $(, $bind:expr)*) => {{
        #[allow(unused_mut)]
        let mut q = ::sqlx::$ctor::<::sqlx::Sqlite $(, $ty)?>($sql);
        $( q = q.bind($bind); )*
        q.$method($exec).await $(.map($map))?
    }};
    (@pg $exec:expr; $ctor:ident [$($ty:ty)?] $method:ident $(=> $map:expr)?; $sql:expr $(, $bind:expr)*) => {{
        #[allow(unused_mut)]
        let mut q = ::sqlx::$ctor::<::sqlx::Postgres $(, $ty)?>(
            ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
        );
        $( q = q.bind($bind); )*
        q.$method($exec).await $(.map($map))?
    }};
}

/// `SELECT` exactly one row as `$ty`.
#[macro_export]
macro_rules! query_one {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(db $db; query_as [$ty] fetch_one; $sql $(, $bind)*)
    };
}

/// `SELECT` zero or one row as `Option<$ty>`.
#[macro_export]
macro_rules! query_opt {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(db $db; query_as [$ty] fetch_optional; $sql $(, $bind)*)
    };
}

/// `SELECT` many rows as `Vec<$ty>`.
#[macro_export]
macro_rules! query_all {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(db $db; query_as [$ty] fetch_all; $sql $(, $bind)*)
    };
}

/// `SELECT` a single scalar column as `$ty` (e.g. `COUNT(*)` as `i64`).
#[macro_export]
macro_rules! query_scalar {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(db $db; query_scalar [$ty] fetch_one; $sql $(, $bind)*)
    };
}

/// Run a statement (INSERT/UPDATE/DELETE) and return rows affected as `u64`.
#[macro_export]
macro_rules! db_execute {
    ($db:expr, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(db $db; query [] execute => |r| r.rows_affected(); $sql $(, $bind)*)
    };
}

/// `query_one!` against `&mut Tx`.
#[macro_export]
macro_rules! query_one_tx {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(tx $tx; query_as [$ty] fetch_one; $sql $(, $bind)*)
    };
}

/// `query_opt!` against `&mut Tx`.
#[macro_export]
macro_rules! query_opt_tx {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(tx $tx; query_as [$ty] fetch_optional; $sql $(, $bind)*)
    };
}

/// `query_all!` against `&mut Tx`.
#[macro_export]
macro_rules! query_all_tx {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(tx $tx; query_as [$ty] fetch_all; $sql $(, $bind)*)
    };
}

/// `query_scalar!` against `&mut Tx`.
#[macro_export]
macro_rules! query_scalar_tx {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(tx $tx; query_scalar [$ty] fetch_one; $sql $(, $bind)*)
    };
}

/// `db_execute!` against `&mut Tx`.
#[macro_export]
macro_rules! db_execute_tx {
    ($tx:expr, $sql:expr $(, $bind:expr)* $(,)?) => {
        $crate::__db_dispatch!(tx $tx; query [] execute => |r| r.rows_affected(); $sql $(, $bind)*)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    // Background must not pass admission while a user op is in flight; the flag
    // is set only after the user op ends.
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

        tokio::task::yield_now().await;
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

    // Regression: concurrent write txs under DEFERRED hit an unretryable
    // SQLITE_BUSY. Needs a file database; in-memory has one connection.
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

        // More transactions than pool connections, so they contend for the writer.
        let mut set = tokio::task::JoinSet::new();
        for i in 0..32_i64 {
            let bg = db.background();
            set.spawn(async move {
                let mut tx = bg.begin().await?;
                if let Tx::Sqlite { tx: sqtx, .. } = &mut tx {
                    // Read then write, as feed sync does (races under DEFERRED).
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

    /// Regression: a migration-added index has no `sqlite_stat1` row, the case
    /// `PRAGMA optimize` acts on. Needs a file database to survive reconnect.
    #[tokio::test]
    async fn connect_refreshes_statistics_for_an_unanalyzed_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite3");
        let path = path.to_str().unwrap();

        let db = Db::connect(path, Backend::Sqlite).await.unwrap();
        if let DbInner::Sqlite(pool) = &db.inner {
            for stmt in [
                "CREATE TABLE probe (id INTEGER PRIMARY KEY, k INTEGER)",
                "CREATE INDEX probe_k ON probe(k)",
                "INSERT INTO probe (k) WITH RECURSIVE s(i) AS \
                 (SELECT 1 UNION ALL SELECT i + 1 FROM s WHERE i < 200) SELECT i FROM s",
                "ANALYZE",
                // Stands in for a migration-added index with no stats.
                "CREATE INDEX probe_k_id ON probe(k, id)",
            ] {
                sqlx::query(stmt).execute(pool).await.unwrap();
            }
        }
        db.shutdown().await;

        let db = Db::connect(path, Backend::Sqlite).await.unwrap();
        let DbInner::Sqlite(pool) = &db.inner else {
            unreachable!("connected with Backend::Sqlite")
        };
        let stat: Option<String> =
            sqlx::query_scalar("SELECT stat FROM sqlite_stat1 WHERE idx = 'probe_k_id'")
                .fetch_optional(pool)
                .await
                .unwrap();

        assert_eq!(
            stat.as_deref(),
            Some("200 1 1"),
            "connecting must leave the new index with statistics the planner can use"
        );
    }
}
