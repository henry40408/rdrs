use std::str::FromStr;
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Executor, PgPool, Postgres, Sqlite, SqlitePool};
use tracing::info;

use crate::config::Backend;

/// Embedded migrations, one set per backend. The two dialects diverge enough
/// (identity columns, timestamp types, `WITHOUT ROWID`, expression-index
/// syntax) that a single migration file cannot serve both; the correct set is
/// selected at connect time by [`Backend`].
static SQLITE_MIGRATOR: Migrator = sqlx::migrate!("migrations/sqlite");
static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

/// A boot-time-selected database handle wrapping a single backend's `sqlx`
/// pool. The backend is chosen once from `DATABASE_URL` (see [`Backend`]) and
/// never changes for the life of the process, so every query dispatches on
/// this two-armed enum via the `query_*!` macros.
///
/// Cloning is cheap: `sqlx` pools are `Arc`-backed handles.
#[derive(Clone)]
pub enum Db {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

/// A backend-tagged transaction, the unit-of-work boundary for operations that
/// compose several model calls atomically (OPML import, entry upserts, `GReader`
/// tag edits). Inner model calls execute against `&mut Tx` via the `*_tx!`
/// macros. Obtained from [`Db::begin`]; finished with [`Tx::commit`] or
/// [`Tx::rollback`] (a dropped `Tx` rolls back).
pub enum Tx<'c> {
    Sqlite(sqlx::Transaction<'c, Sqlite>),
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
                Db::Sqlite(pool)
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
                Db::Postgres(pool)
            }
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
        let db = Db::Sqlite(pool);
        db.migrate().await?;
        Ok(db)
    }

    /// Run the backend's embedded migrations. Migrations use `IF NOT EXISTS`,
    /// so an existing (pre-sqlx) `SQLite` database is baselined harmlessly: the
    /// consolidated `0001` no-ops against already-present tables and is recorded
    /// in `_sqlx_migrations`.
    async fn migrate(&self) -> Result<(), sqlx::Error> {
        match self {
            Db::Sqlite(pool) => SQLITE_MIGRATOR.run(pool).await,
            Db::Postgres(pool) => POSTGRES_MIGRATOR.run(pool).await,
        }
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))
    }

    /// Begin a transaction on the underlying pool.
    pub async fn begin(&self) -> Result<Tx<'_>, sqlx::Error> {
        Ok(match self {
            Db::Sqlite(pool) => Tx::Sqlite(pool.begin().await?),
            Db::Postgres(pool) => Tx::Postgres(pool.begin().await?),
        })
    }

    /// Flush and close the pool. For `SQLite` this truncates the WAL first so no
    /// `-wal`/`-shm` sidecars linger after shutdown.
    pub async fn shutdown(&self) {
        if let Db::Sqlite(pool) = self {
            info!("Executing WAL checkpoint before shutdown...");
            if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE);")
                .execute(pool)
                .await
            {
                tracing::error!("WAL checkpoint failed: {e}");
            }
        }
        match self {
            Db::Sqlite(pool) => pool.close().await,
            Db::Postgres(pool) => pool.close().await,
        }
    }
}

impl Tx<'_> {
    /// Commit the transaction.
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        match self {
            Tx::Sqlite(t) => t.commit().await,
            Tx::Postgres(t) => t.commit().await,
        }
    }

    /// Roll the transaction back explicitly (dropping also rolls back).
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        match self {
            Tx::Sqlite(t) => t.rollback().await,
            Tx::Postgres(t) => t.rollback().await,
        }
    }
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Db::Sqlite(_) => f.write_str("Db::Sqlite"),
            Db::Postgres(_) => f.write_str("Db::Postgres"),
        }
    }
}

/// `true` if `e` is a UNIQUE / primary-key constraint violation, on either
/// backend. Model layers use this to translate a duplicate insert into a domain
/// error (e.g. `AppError::CategoryExists`).
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.kind() == sqlx::error::ErrorKind::UniqueViolation)
}

/// Rewrite the SQLite-dialect fragments in a macro `$sql` literal to their
/// PostgreSQL equivalents at dispatch time. Applied *only* in the Postgres arm
/// of the `query_*!` / `db_execute!` macros so a model writes one SQL literal
/// that runs correctly on both backends.
///
/// Currently rewrites the SQLite scalar `datetime('now')` — which yields the
/// `%Y-%m-%d %H:%M:%S` TEXT that the composite pagination cursor and the
/// timestamp-column DEFAULTs depend on — to PostgreSQL `now()` (a `timestamptz`
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

/// `SELECT` exactly one row as `$ty`.
#[macro_export]
macro_rules! query_one {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $db {
            $crate::db::Db::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_one(pool).await
            }
            $crate::db::Db::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_one(pool).await
            }
        }
    }};
}

/// `SELECT` zero or one row as `Option<$ty>`.
#[macro_export]
macro_rules! query_opt {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $db {
            $crate::db::Db::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_optional(pool).await
            }
            $crate::db::Db::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_optional(pool).await
            }
        }
    }};
}

/// `SELECT` many rows as `Vec<$ty>`.
#[macro_export]
macro_rules! query_all {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $db {
            $crate::db::Db::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_all(pool).await
            }
            $crate::db::Db::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_as::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_all(pool).await
            }
        }
    }};
}

/// `SELECT` a single scalar column as `$ty` (e.g. `COUNT(*)` as `i64`).
#[macro_export]
macro_rules! query_scalar {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $db {
            $crate::db::Db::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_scalar::<::sqlx::Sqlite, $ty>($sql);
                $( q = q.bind($bind); )*
                q.fetch_one(pool).await
            }
            $crate::db::Db::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query_scalar::<::sqlx::Postgres, $ty>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.fetch_one(pool).await
            }
        }
    }};
}

/// Run a statement (INSERT/UPDATE/DELETE) and return rows affected as `u64`.
#[macro_export]
macro_rules! db_execute {
    ($db:expr, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $db {
            $crate::db::Db::Sqlite(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query::<::sqlx::Sqlite>($sql);
                $( q = q.bind($bind); )*
                q.execute(pool).await.map(|r| r.rows_affected())
            }
            $crate::db::Db::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut q = ::sqlx::query::<::sqlx::Postgres>(
                    ::sqlx::AssertSqlSafe($crate::db::pg_rewrite($sql)),
                );
                $( q = q.bind($bind); )*
                q.execute(pool).await.map(|r| r.rows_affected())
            }
        }
    }};
}

/// `query_one!` against `&mut Tx`.
#[macro_export]
macro_rules! query_one_tx {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $tx {
            $crate::db::Tx::Sqlite(t) => {
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
            $crate::db::Tx::Sqlite(t) => {
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
            $crate::db::Tx::Sqlite(t) => {
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
            $crate::db::Tx::Sqlite(t) => {
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
            $crate::db::Tx::Sqlite(t) => {
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
