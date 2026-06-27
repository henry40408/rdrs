use std::fmt;
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

/// Timeout for waiting on the database actor to respond (30s)
const DB_EXECUTE_TIMEOUT: Duration = Duration::from_secs(30);

/// Priority level for database operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbPriority {
    /// User-facing requests (handlers, middleware). Processed first.
    User,
    /// Background tasks (feed sync, summary worker, cleanup). Processed when no user work pending.
    Background,
}

/// Error type for `DbPool` operations.
#[derive(Debug)]
pub enum DbError {
    /// The actor task has stopped; the connection is no longer available.
    ActorStopped,
    /// Timed out waiting for the database actor to respond.
    Timeout,
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::ActorStopped => write!(f, "Database actor has stopped"),
            DbError::Timeout => write!(f, "Database operation timed out"),
        }
    }
}

impl std::error::Error for DbError {}

type BoxedDbFn = Box<dyn FnOnce(&Connection) -> Box<dyn std::any::Any + Send> + Send>;

struct DbMessage {
    work: BoxedDbFn,
    respond: oneshot::Sender<Box<dyn std::any::Any + Send>>,
}

/// A prioritized database connection pool backed by `SQLite` connections.
///
/// All database access goes through actor tasks that own the `Connection`s.
/// User-priority work is always processed before background-priority work.
///
/// Supports separate write and read connections for better concurrency under
/// WAL mode. Read operations can proceed concurrently with writes.
#[derive(Clone)]
pub struct DbPool {
    // Write connection channels
    user_tx: mpsc::Sender<DbMessage>,
    bg_tx: mpsc::Sender<DbMessage>,
    // Read-only connection channels
    read_user_tx: mpsc::Sender<DbMessage>,
    read_bg_tx: mpsc::Sender<DbMessage>,
}

impl DbPool {
    /// Create a new `DbPool`, spawning actor tasks for both write and read connections.
    ///
    /// Enables WAL mode on the write connection and sets the read connection
    /// to query-only mode for safety. Returns the `DbPool` and the `JoinHandle`
    /// for the combined actor tasks.
    pub fn new(write_conn: Connection, read_conn: Connection) -> (Self, JoinHandle<()>) {
        // Enable WAL mode on write connection
        match write_conn.execute_batch("PRAGMA journal_mode=WAL;") {
            Err(e) => {
                error!("Failed to enable WAL mode: {}", e);
            }
            _ => {
                debug!("SQLite WAL mode enabled");
            }
        }

        // Set read connection to query-only mode for safety
        match read_conn.execute_batch("PRAGMA query_only=ON;") {
            Err(e) => {
                error!("Failed to enable query_only mode on read connection: {}", e);
            }
            _ => {
                debug!("Read connection query_only mode enabled");
            }
        }

        // Tuning pragmas applied to both connections. synchronous=NORMAL is
        // safe under WAL (durability bound to checkpoints instead of every
        // commit) and skips a per-txn fsync; cache_size=-20000 reserves
        // 20 MiB of page cache per connection; mmap_size=128 MiB lets reads
        // bypass the syscall path for hot pages; temp_store=MEMORY keeps
        // sorts/temp indexes in RAM; busy_timeout=5s prevents instant SQLITE_BUSY
        // when the actor briefly contends with a checkpoint.
        apply_tuning_pragmas(&write_conn, "write");
        apply_tuning_pragmas(&read_conn, "read");

        // Write connection channels
        let (user_tx, user_rx) = mpsc::channel::<DbMessage>(256);
        let (bg_tx, bg_rx) = mpsc::channel::<DbMessage>(64);

        // Read connection channels
        let (read_user_tx, read_user_rx) = mpsc::channel::<DbMessage>(256);
        let (read_bg_tx, read_bg_rx) = mpsc::channel::<DbMessage>(64);

        let handle = tokio::spawn(async move {
            tokio::join!(
                actor_loop(write_conn, user_rx, bg_rx),
                actor_loop(read_conn, read_user_rx, read_bg_rx),
            );
        });

        (
            DbPool {
                user_tx,
                bg_tx,
                read_user_tx,
                read_bg_tx,
            },
            handle,
        )
    }

    /// Gracefully shutdown the database connection.
    ///
    /// Executes a WAL checkpoint to clean up shm/wal files before closing.
    pub async fn shutdown(self) -> Result<(), DbError> {
        info!("Executing WAL checkpoint before shutdown...");
        let checkpoint_result: Result<(), rusqlite::Error> = self
            .user(|conn| conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);"))
            .await?;
        if let Err(e) = checkpoint_result {
            error!("WAL checkpoint failed: {}", e);
        } else {
            info!("WAL checkpoint completed");
        }
        // Drop channels to let actor exit
        drop(self);
        Ok(())
    }

    /// Execute a closure on the database connection with the given priority.
    ///
    /// The closure receives a `&Connection` and returns a value of type `T`.
    /// Returns `Err(DbError::ActorStopped)` if the actor has shut down.
    pub async fn execute<F, T>(&self, priority: DbPriority, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> T + Send + 'static,
        T: Send + 'static,
    {
        let (resp_tx, resp_rx) = oneshot::channel();

        let msg = DbMessage {
            work: Box::new(move |conn| {
                let result = f(conn);
                Box::new(result) as Box<dyn std::any::Any + Send>
            }),
            respond: resp_tx,
        };

        let tx = match priority {
            DbPriority::User => &self.user_tx,
            DbPriority::Background => &self.bg_tx,
        };

        tx.send(msg).await.map_err(|_e| DbError::ActorStopped)?;

        let boxed = tokio::time::timeout(DB_EXECUTE_TIMEOUT, resp_rx)
            .await
            .map_err(|_e| DbError::Timeout)?
            .map_err(|_e| DbError::ActorStopped)?;

        // Downcast back to T
        Ok(*boxed.downcast::<T>().expect("DbPool type mismatch"))
    }

    /// Execute a closure with User priority (for handlers and middleware).
    pub async fn user<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> T + Send + 'static,
        T: Send + 'static,
    {
        self.execute(DbPriority::User, f).await
    }

    /// Execute a closure with Background priority (for sync, workers, cleanup).
    pub async fn background<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> T + Send + 'static,
        T: Send + 'static,
    {
        self.execute(DbPriority::Background, f).await
    }

    /// Enqueue a write-actor closure with User priority and return
    /// immediately, WITHOUT awaiting its result. Ordering is preserved (single
    /// FIFO `user_tx`), so rapid star→unstar still applies in submission order.
    ///
    /// Fire-and-forget: the closure must log its own errors (the caller cannot
    /// observe success/failure). Used for optimistic state-flip writes whose
    /// HTTP response is rendered before the write lands; a dropped write
    /// self-heals on the next sidebar poll / page reload.
    pub fn user_detached<F>(&self, f: F)
    where
        F: FnOnce(&Connection) + Send + 'static,
    {
        // The response receiver is dropped immediately; the actor's
        // `msg.respond.send(...)` then fails silently (already handled in
        // `process_message`). `try_send` never blocks the caller.
        let (resp_tx, _resp_rx) = oneshot::channel();
        let msg = DbMessage {
            work: Box::new(move |conn| {
                f(conn);
                Box::new(()) as Box<dyn std::any::Any + Send>
            }),
            respond: resp_tx,
        };
        match self.user_tx.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                error!("user_detached: write queue full; dropping write (self-heals on next poll)");
            }
            Err(TrySendError::Closed(_)) => {
                error!("user_detached: db actor stopped; write dropped");
            }
        }
    }

    /// Execute a read-only closure on the read connection with the given priority.
    async fn execute_read<F, T>(&self, priority: DbPriority, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> T + Send + 'static,
        T: Send + 'static,
    {
        let (resp_tx, resp_rx) = oneshot::channel();

        let msg = DbMessage {
            work: Box::new(move |conn| {
                let result = f(conn);
                Box::new(result) as Box<dyn std::any::Any + Send>
            }),
            respond: resp_tx,
        };

        let tx = match priority {
            DbPriority::User => &self.read_user_tx,
            DbPriority::Background => &self.read_bg_tx,
        };

        tx.send(msg).await.map_err(|_e| DbError::ActorStopped)?;

        let boxed = tokio::time::timeout(DB_EXECUTE_TIMEOUT, resp_rx)
            .await
            .map_err(|_e| DbError::Timeout)?
            .map_err(|_e| DbError::ActorStopped)?;

        Ok(*boxed.downcast::<T>().expect("DbPool type mismatch"))
    }

    /// Execute a read-only closure with User priority on the read connection.
    pub async fn read_user<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> T + Send + 'static,
        T: Send + 'static,
    {
        self.execute_read(DbPriority::User, f).await
    }

    /// Execute a read-only closure with Background priority on the read connection.
    pub async fn read_background<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> T + Send + 'static,
        T: Send + 'static,
    {
        self.execute_read(DbPriority::Background, f).await
    }
}

/// The actor loop that owns the Connection and processes messages.
///
/// Uses `biased` select to always drain user messages before background ones.
async fn actor_loop(
    conn: Connection,
    mut user_rx: mpsc::Receiver<DbMessage>,
    mut bg_rx: mpsc::Receiver<DbMessage>,
) {
    debug!("Database actor started");

    loop {
        // Use biased select: always prefer user channel
        tokio::select! {
            biased;

            msg = user_rx.recv() => {
                if let Some(msg) = msg { process_message(&conn, msg) } else {
                    // User channel closed — drain background and exit
                    while let Ok(msg) = bg_rx.try_recv() {
                        process_message(&conn, msg);
                    }
                    break;
                }
                // After processing one user message, drain any remaining user messages
                while let Ok(msg) = user_rx.try_recv() {
                    process_message(&conn, msg);
                }
            }

            msg = bg_rx.recv() => {
                if let Some(msg) = msg { process_message(&conn, msg) } else {
                    // Background channel closed — continue with user only
                    while let Some(msg) = user_rx.recv().await {
                        process_message(&conn, msg);
                    }
                    break;
                }
            }
        }
    }

    debug!("Database actor stopped");
}

fn process_message(conn: &Connection, msg: DbMessage) {
    let result = (msg.work)(conn);
    // If the receiver is dropped, we just discard the result
    let _ = msg.respond.send(result);
}

const TUNING_PRAGMAS: &str = "\
    PRAGMA synchronous=NORMAL;\
    PRAGMA cache_size=-20000;\
    PRAGMA mmap_size=134217728;\
    PRAGMA temp_store=MEMORY;\
    PRAGMA busy_timeout=5000;\
";

fn apply_tuning_pragmas(conn: &Connection, label: &str) {
    match conn.execute_batch(TUNING_PRAGMAS) {
        Err(e) => {
            error!(
                "Failed to apply tuning pragmas on {} connection: {}",
                label, e
            );
        }
        _ => {
            debug!("SQLite tuning pragmas applied on {} connection", label);
        }
    }
}

impl fmt::Debug for DbPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbPool").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_user_execute() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT);")
            .unwrap();

        let (pool, _handle) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        let result = pool
            .user(|conn| {
                conn.execute("INSERT INTO test (value) VALUES (?1)", ["hello"])
                    .unwrap();
                conn.query_row("SELECT value FROM test WHERE id = 1", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap()
            })
            .await
            .unwrap();

        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_background_execute() {
        let conn = Connection::open_in_memory().unwrap();
        let (pool, _handle) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        let result = pool
            .background(|conn| {
                conn.execute_batch("CREATE TABLE bg_test (id INTEGER);")
                    .unwrap();
                42
            })
            .await
            .unwrap();

        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_user_priority_over_background() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ordering (seq INTEGER);")
            .unwrap();

        let (pool, _handle) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        // Send several user and background tasks
        let mut handles = vec![];

        for i in 0..5 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                pool.user(move |conn| {
                    conn.execute("INSERT INTO ordering (seq) VALUES (?1)", [i])
                        .unwrap();
                })
                .await
                .unwrap();
            }));
        }

        for i in 100..105 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                pool.background(move |conn| {
                    conn.execute("INSERT INTO ordering (seq) VALUES (?1)", [i])
                        .unwrap();
                })
                .await
                .unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // Verify all tasks completed
        let count = pool
            .user(|conn| {
                conn.query_row("SELECT COUNT(*) FROM ordering", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
            })
            .await
            .unwrap();

        assert_eq!(count, 10);
    }

    #[tokio::test]
    async fn test_error_propagation() {
        let conn = Connection::open_in_memory().unwrap();
        let (pool, _handle) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        let result: Result<Result<String, rusqlite::Error>, DbError> = pool
            .user(|conn| {
                conn.query_row("SELECT * FROM nonexistent", [], |row| {
                    row.get::<_, String>(0)
                })
            })
            .await;

        // DbPool execute succeeds, but the inner result is a rusqlite error
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }

    #[tokio::test]
    async fn test_multiple_sequential_operations() {
        let conn = Connection::open_in_memory().unwrap();
        let (pool, _handle) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        pool.user(|conn| {
            conn.execute_batch("CREATE TABLE multi (id INTEGER PRIMARY KEY, val INTEGER);")
                .unwrap();
        })
        .await
        .unwrap();

        for i in 0..10 {
            pool.user(move |conn| {
                conn.execute("INSERT INTO multi (val) VALUES (?1)", [i])
                    .unwrap();
            })
            .await
            .unwrap();
        }

        let count = pool
            .user(|conn| {
                conn.query_row("SELECT COUNT(*) FROM multi", [], |row| row.get::<_, i64>(0))
                    .unwrap()
            })
            .await
            .unwrap();

        assert_eq!(count, 10);
    }

    #[test]
    fn test_dberror_display() {
        let err = DbError::ActorStopped;
        assert_eq!(format!("{}", err), "Database actor has stopped");

        let err = DbError::Timeout;
        assert_eq!(format!("{}", err), "Database operation timed out");
    }

    #[test]
    fn test_dbpool_debug() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (pool, _handle) = rt.block_on(async {
            let conn = Connection::open_in_memory().unwrap();
            DbPool::new(conn, Connection::open_in_memory().unwrap())
        });
        let debug_str = format!("{:?}", pool);
        assert!(debug_str.contains("DbPool"));
    }

    #[tokio::test]
    async fn test_actor_stops_when_pool_dropped() {
        // Create a pool and immediately extract a clone of the senders
        // so we can attempt to use them after the actor stops.
        let conn = Connection::open_in_memory().unwrap();
        let (user_tx, user_rx) = mpsc::channel::<DbMessage>(256);
        let (bg_tx, bg_rx) = mpsc::channel::<DbMessage>(64);

        tokio::spawn(actor_loop(conn, user_rx, bg_rx));

        // Verify the actor works
        let (resp_tx, resp_rx) = oneshot::channel();
        user_tx
            .send(DbMessage {
                work: Box::new(|_conn| Box::new(42i32) as Box<dyn std::any::Any + Send>),
                respond: resp_tx,
            })
            .await
            .unwrap();
        let result = resp_rx.await.unwrap();
        assert_eq!(*result.downcast::<i32>().unwrap(), 42);

        // Drop the user_tx — this closes the user channel, causing actor to
        // drain background and exit.
        drop(user_tx);

        // Give the actor time to exit
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // bg channel send should now fail because the actor has stopped
        let (resp_tx2, _resp_rx2) = oneshot::channel();
        let send_result = bg_tx
            .send(DbMessage {
                work: Box::new(|_conn| Box::new(()) as Box<dyn std::any::Any + Send>),
                respond: resp_tx2,
            })
            .await;
        assert!(send_result.is_err());
    }

    #[tokio::test]
    async fn test_shutdown_executes_wal_checkpoint() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE shutdown_test (id INTEGER PRIMARY KEY);")
            .unwrap();

        let (pool, handle) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        // Insert some data
        pool.user(|conn| {
            conn.execute("INSERT INTO shutdown_test (id) VALUES (1)", [])
                .unwrap();
        })
        .await
        .unwrap();

        // Shutdown should complete successfully
        let result = pool.shutdown().await;
        assert!(result.is_ok());

        // Actor should exit after shutdown
        let join_result = handle.await;
        assert!(join_result.is_ok());
    }

    #[tokio::test]
    async fn test_tuning_pragmas_applied_to_both_connections() {
        // Use file-backed connections so mmap_size is honored — memory dbs
        // report 0 for mmap_size regardless of what's set.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let write_conn = Connection::open(&path).unwrap();
        let read_conn = Connection::open(&path).unwrap();

        let (pool, _handle) = DbPool::new(write_conn, read_conn);

        fn read_pragmas(conn: &Connection) -> (i64, i64, i64, i64, i64) {
            let sync: i64 = conn
                .pragma_query_value(None, "synchronous", |r| r.get(0))
                .unwrap();
            let cache: i64 = conn
                .pragma_query_value(None, "cache_size", |r| r.get(0))
                .unwrap();
            let mmap: i64 = conn
                .pragma_query_value(None, "mmap_size", |r| r.get(0))
                .unwrap();
            let temp: i64 = conn
                .pragma_query_value(None, "temp_store", |r| r.get(0))
                .unwrap();
            let busy: i64 = conn
                .pragma_query_value(None, "busy_timeout", |r| r.get(0))
                .unwrap();
            (sync, cache, mmap, temp, busy)
        }

        fn assert_tuned(label: &str, values: (i64, i64, i64, i64, i64)) {
            let (sync, cache, mmap, temp, busy) = values;
            assert_eq!(
                sync, 1,
                "synchronous should be NORMAL (1) on {} conn",
                label
            );
            assert_eq!(cache, -20000, "cache_size on {} conn", label);
            assert_eq!(mmap, 134217728, "mmap_size on {} conn", label);
            assert_eq!(temp, 2, "temp_store should be MEMORY (2) on {} conn", label);
            assert_eq!(busy, 5000, "busy_timeout on {} conn", label);
        }

        let write_values = pool.user(read_pragmas).await.unwrap();
        assert_tuned("write", write_values);

        let read_values = pool.read_user(read_pragmas).await.unwrap();
        assert_tuned("read", read_values);
    }

    #[tokio::test]
    async fn test_send_fails_after_receiver_dropped() {
        // Test that sending fails when the receiver has been dropped
        let conn = Connection::open_in_memory().unwrap();
        let (user_tx, user_rx) = mpsc::channel::<DbMessage>(256);
        let (_bg_tx, bg_rx) = mpsc::channel::<DbMessage>(64);

        let handle = tokio::spawn(actor_loop(conn, user_rx, bg_rx));

        // Drop sender to close user channel (actor will exit after draining bg)
        drop(user_tx);

        // Wait for actor to stop
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_db_priority_debug() {
        // Test Debug implementation for DbPriority
        let user = DbPriority::User;
        let bg = DbPriority::Background;
        assert_eq!(format!("{:?}", user), "User");
        assert_eq!(format!("{:?}", bg), "Background");
    }

    #[tokio::test]
    async fn test_db_priority_clone_and_eq() {
        let user1 = DbPriority::User;
        let user2 = user1;
        assert_eq!(user1, user2);

        let bg = DbPriority::Background;
        assert_ne!(user1, bg);
    }

    #[tokio::test]
    async fn test_dberror_is_error_trait() {
        let err = DbError::ActorStopped;
        // Verify it implements std::error::Error
        let _: &dyn std::error::Error = &err;
    }

    /// Helper to open a shared in-memory `SQLite` connection by URI name.
    fn open_shared_memory(name: &str) -> Connection {
        let uri = format!("file:{}?mode=memory&cache=shared", name);
        Connection::open_with_flags(
            uri,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_read_user_execute() {
        // Use shared in-memory DB so both write and read connections see the same data
        let wc = open_shared_memory("test_read_user");
        wc.execute_batch("CREATE TABLE read_test (id INTEGER PRIMARY KEY, value TEXT);")
            .unwrap();
        wc.execute("INSERT INTO read_test (id, value) VALUES (1, 'hello')", [])
            .unwrap();

        let rc = open_shared_memory("test_read_user");
        let (pool, _handle) = DbPool::new(wc, rc);

        let result = pool
            .read_user(|conn| {
                conn.query_row("SELECT value FROM read_test WHERE id = 1", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap()
            })
            .await
            .unwrap();

        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_read_background_execute() {
        let wc = open_shared_memory("test_read_bg");
        wc.execute_batch("CREATE TABLE read_bg_test (id INTEGER PRIMARY KEY, value TEXT);")
            .unwrap();
        wc.execute(
            "INSERT INTO read_bg_test (id, value) VALUES (1, 'world')",
            [],
        )
        .unwrap();

        let rc = open_shared_memory("test_read_bg");
        let (pool, _handle) = DbPool::new(wc, rc);

        let result = pool
            .read_background(|conn| {
                conn.query_row("SELECT value FROM read_bg_test WHERE id = 1", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap()
            })
            .await
            .unwrap();

        assert_eq!(result, "world");
    }

    #[tokio::test]
    async fn test_background_channel_closes_actor_continues() {
        // Test the case where background channel closes but user channel is still open
        let conn = Connection::open_in_memory().unwrap();
        let (user_tx, user_rx) = mpsc::channel::<DbMessage>(256);
        let (bg_tx, bg_rx) = mpsc::channel::<DbMessage>(64);

        let handle = tokio::spawn(actor_loop(conn, user_rx, bg_rx));

        // Drop the background channel
        drop(bg_tx);

        // User channel should still work
        let (resp_tx, resp_rx) = oneshot::channel();
        user_tx
            .send(DbMessage {
                work: Box::new(|_conn| Box::new(123i32) as Box<dyn std::any::Any + Send>),
                respond: resp_tx,
            })
            .await
            .unwrap();
        let result = resp_rx.await.unwrap();
        assert_eq!(*result.downcast::<i32>().unwrap(), 123);

        // Close user channel and wait for actor to exit
        drop(user_tx);
        let join_result = handle.await;
        assert!(join_result.is_ok());
    }

    #[tokio::test]
    async fn test_user_detached_eventually_applies() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE d (id INTEGER PRIMARY KEY, v INTEGER);")
            .unwrap();
        let (pool, _h) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        // Fire-and-forget: returns immediately (no .await on the write itself).
        pool.user_detached(|conn| {
            conn.execute("INSERT INTO d (v) VALUES (1)", []).unwrap();
        });

        // Flush via a FIFO sentinel: a subsequent user() call runs on the same
        // write actor AFTER the detached write, so the row is guaranteed present.
        let count = pool
            .user(|conn| {
                conn.query_row("SELECT COUNT(*) FROM d", [], |r| r.get::<_, i64>(0))
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_user_detached_preserves_submission_order() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE seq (n INTEGER);").unwrap();
        let (pool, _h) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        for i in 0..5 {
            pool.user_detached(move |conn| {
                conn.execute("INSERT INTO seq (n) VALUES (?1)", [i])
                    .unwrap();
            });
        }

        // Sentinel flush, then assert FIFO order (0,1,2,3,4).
        let rows: Vec<i64> = pool
            .user(|conn| {
                let mut stmt = conn.prepare("SELECT n FROM seq ORDER BY rowid").unwrap();
                stmt.query_map([], |r| r.get::<_, i64>(0))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(rows, vec![0, 1, 2, 3, 4]);
    }
}
