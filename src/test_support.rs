//! Fixtures shared by the unit tests of more than one module.

use crate::db::Db;
use crate::models::user::{self, Role, User};

pub async fn setup_db() -> Db {
    Db::connect_in_memory().await.unwrap()
}

/// A user whose password hash is a placeholder — enough for anything short of
/// actually signing in.
pub async fn seed_user(db: &Db, username: &str, role: Role) -> User {
    user::create_user(db, username, "hash", role).await.unwrap()
}

pub async fn create_test_user(db: &Db, username: &str) -> i64 {
    seed_user(db, username, Role::User).await.id
}
