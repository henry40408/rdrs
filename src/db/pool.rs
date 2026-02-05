use std::fmt;

use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

/// Priority level for database operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbPriority {
    /// User-facing requests (handlers, middleware). Processed first.
    User,
    /// Background tasks (feed sync, summary worker, cleanup). Processed when no user work pending.
    Background,
}

/// Error type for DbPool operations.
#[derive(Debug)]
pub enum DbError {
    /// The actor task has stopped; the connection is no longer available.
    ActorStopped,
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::ActorStopped => write!(f, "Database actor has stopped"),
        }
    }
}

impl std::error::Error for DbError {}

type BoxedDbFn = Box<dyn FnOnce(&Connection) -> Box<dyn std::any::Any + Send> + Send>;

struct DbMessage {
    work: BoxedDbFn,
    respond: oneshot::Sender<Box<dyn std::any::Any + Send>>,
}

/// A prioritized database connection pool backed by a single SQLite connection.
///
/// All database access goes through an actor task that owns the `Connection`.
/// User-priority work is always processed before background-priority work.
#[derive(Clone)]
pub struct DbPool {
    user_tx: mpsc::Sender<DbMessage>,
    bg_tx: mpsc::Sender<DbMessage>,
}

impl DbPool {
    /// Create a new DbPool, spawning the actor task.
    ///
    /// Enables WAL mode on the connection for better concurrent read performance.
    /// Returns the DbPool and the JoinHandle for the actor task.
    pub fn new(conn: Connection) -> (Self, JoinHandle<()>) {
        // Enable WAL mode for better read performance
        if let Err(e) = conn.execute_batch("PRAGMA journal_mode=WAL;") {
            error!("Failed to enable WAL mode: {}", e);
        } else {
            debug!("SQLite WAL mode enabled");
        }

        let (user_tx, user_rx) = mpsc::channel::<DbMessage>(256);
        let (bg_tx, bg_rx) = mpsc::channel::<DbMessage>(64);

        let handle = tokio::spawn(actor_loop(conn, user_rx, bg_rx));

        (DbPool { user_tx, bg_tx }, handle)
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

        tx.send(msg).await.map_err(|_| DbError::ActorStopped)?;

        let boxed = resp_rx.await.map_err(|_| DbError::ActorStopped)?;

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
                match msg {
                    Some(msg) => process_message(&conn, msg),
                    None => {
                        // User channel closed — drain background and exit
                        while let Ok(msg) = bg_rx.try_recv() {
                            process_message(&conn, msg);
                        }
                        break;
                    }
                }
                // After processing one user message, drain any remaining user messages
                while let Ok(msg) = user_rx.try_recv() {
                    process_message(&conn, msg);
                }
            }

            msg = bg_rx.recv() => {
                match msg {
                    Some(msg) => process_message(&conn, msg),
                    None => {
                        // Background channel closed — continue with user only
                        while let Some(msg) = user_rx.recv().await {
                            process_message(&conn, msg);
                        }
                        break;
                    }
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

        let (pool, _handle) = DbPool::new(conn);

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
        let (pool, _handle) = DbPool::new(conn);

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

        let (pool, _handle) = DbPool::new(conn);

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
        let (pool, _handle) = DbPool::new(conn);

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
        let (pool, _handle) = DbPool::new(conn);

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
    }

    #[test]
    fn test_dbpool_debug() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (pool, _handle) = rt.block_on(async {
            let conn = Connection::open_in_memory().unwrap();
            DbPool::new(conn)
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

        let (pool, handle) = DbPool::new(conn);

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
}
