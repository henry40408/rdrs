use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::{Db, is_unique_violation};
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_one, query_opt, query_scalar};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Role::Admin),
            "user" => Ok(Role::User),
            _ => Err(()),
        }
    }
}

// `role` is stored as a TEXT column on both backends. We keep the public struct
// field as `Role` (so callers are unaffected) by teaching sqlx to treat it as a
// string: `Type`/`Decode` delegate to `String`, mapping the stored text back to
// the enum. Binds always pass `role.as_str()`, so no `Encode` impl is needed.
impl sqlx::Type<sqlx::Sqlite> for Role {
    fn type_info() -> <sqlx::Sqlite as sqlx::Database>::TypeInfo {
        <String as sqlx::Type<sqlx::Sqlite>>::type_info()
    }
    fn compatible(ty: &<sqlx::Sqlite as sqlx::Database>::TypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for Role {
    fn decode(
        value: <sqlx::Sqlite as sqlx::Database>::ValueRef<'r>,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Sqlite>>::decode(value)?;
        Ok(Role::from_str(&s).unwrap_or(Role::User))
    }
}

impl sqlx::Type<sqlx::Postgres> for Role {
    fn type_info() -> <sqlx::Postgres as sqlx::Database>::TypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
    fn compatible(ty: &<sqlx::Postgres as sqlx::Database>::TypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Role {
    fn decode(
        value: <sqlx::Postgres as sqlx::Database>::ValueRef<'r>,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(Role::from_str(&s).unwrap_or(Role::User))
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: Role,
    pub disabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl User {
    pub fn is_disabled(&self) -> bool {
        self.disabled_at.is_some()
    }

    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
}

pub async fn create_user(
    db: &Db,
    username: &str,
    password_hash: &str,
    role: Role,
) -> AppResult<User> {
    query_one!(
        db,
        User,
        "INSERT INTO \"user\" (username, password_hash, role) VALUES ($1, $2, $3) \
         RETURNING id, username, password_hash, role, disabled_at, created_at",
        username,
        password_hash,
        role.as_str()
    )
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::UsernameExists
        } else {
            AppError::Database(e)
        }
    })
}

pub async fn find_by_username(db: &Db, username: &str) -> AppResult<Option<User>> {
    query_opt!(
        db,
        User,
        "SELECT id, username, password_hash, role, disabled_at, created_at \
         FROM \"user\" WHERE username = $1",
        username
    )
    .map_err(AppError::Database)
}

pub async fn find_by_id(db: &Db, id: i64) -> AppResult<Option<User>> {
    query_opt!(
        db,
        User,
        "SELECT id, username, password_hash, role, disabled_at, created_at \
         FROM \"user\" WHERE id = $1",
        id
    )
    .map_err(AppError::Database)
}

pub async fn list_all(db: &Db) -> AppResult<Vec<User>> {
    query_all!(
        db,
        User,
        "SELECT id, username, password_hash, role, disabled_at, created_at \
         FROM \"user\" ORDER BY id"
    )
    .map_err(AppError::Database)
}

pub async fn update_password(db: &Db, user_id: i64, new_password_hash: &str) -> AppResult<()> {
    let rows = db_execute!(
        db,
        "UPDATE \"user\" SET password_hash = $1 WHERE id = $2",
        new_password_hash,
        user_id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub async fn update_role(db: &Db, user_id: i64, role: Role) -> AppResult<()> {
    let rows = db_execute!(
        db,
        "UPDATE \"user\" SET role = $1 WHERE id = $2",
        role.as_str(),
        user_id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub async fn disable_user(db: &Db, user_id: i64) -> AppResult<()> {
    let rows = db_execute!(
        db,
        "UPDATE \"user\" SET disabled_at = $1 WHERE id = $2",
        Utc::now(),
        user_id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub async fn enable_user(db: &Db, user_id: i64) -> AppResult<()> {
    let rows = db_execute!(
        db,
        "UPDATE \"user\" SET disabled_at = NULL WHERE id = $1",
        user_id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub async fn delete_user(db: &Db, user_id: i64) -> AppResult<()> {
    let rows =
        db_execute!(db, "DELETE FROM user WHERE id = $1", user_id).map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub async fn count(db: &Db) -> AppResult<i64> {
    query_scalar!(db, i64, "SELECT COUNT(*) FROM \"user\"").map_err(AppError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_create_and_find_user() {
        let db = setup_db().await;

        let user = create_user(&db, "testuser", "hash123", Role::User)
            .await
            .unwrap();
        assert_eq!(user.username, "testuser");
        assert_eq!(user.role, Role::User);
        assert!(!user.is_disabled());

        let found = find_by_username(&db, "testuser").await.unwrap().unwrap();
        assert_eq!(found.id, user.id);

        let found_by_id = find_by_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(found_by_id.username, "testuser");
    }

    #[tokio::test]
    async fn test_duplicate_username() {
        let db = setup_db().await;

        create_user(&db, "testuser", "hash123", Role::User)
            .await
            .unwrap();
        let result = create_user(&db, "testuser", "hash456", Role::User).await;
        assert!(matches!(result, Err(AppError::UsernameExists)));
    }

    #[tokio::test]
    async fn test_disable_enable_user() {
        let db = setup_db().await;

        let user = create_user(&db, "testuser", "hash123", Role::User)
            .await
            .unwrap();
        assert!(!user.is_disabled());

        disable_user(&db, user.id).await.unwrap();
        let disabled = find_by_id(&db, user.id).await.unwrap().unwrap();
        assert!(disabled.is_disabled());

        enable_user(&db, user.id).await.unwrap();
        let enabled = find_by_id(&db, user.id).await.unwrap().unwrap();
        assert!(!enabled.is_disabled());
    }

    #[tokio::test]
    async fn test_update_role() {
        let db = setup_db().await;

        let user = create_user(&db, "testuser", "hash123", Role::User)
            .await
            .unwrap();
        assert_eq!(user.role, Role::User);

        update_role(&db, user.id, Role::Admin).await.unwrap();
        let admin = find_by_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(admin.role, Role::Admin);
    }

    #[tokio::test]
    async fn test_delete_user() {
        let db = setup_db().await;

        let user = create_user(&db, "testuser", "hash123", Role::User)
            .await
            .unwrap();
        assert_eq!(count(&db).await.unwrap(), 1);

        delete_user(&db, user.id).await.unwrap();
        assert_eq!(count(&db).await.unwrap(), 0);
        assert!(find_by_id(&db, user.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_list_all() {
        let db = setup_db().await;

        create_user(&db, "user1", "hash1", Role::Admin)
            .await
            .unwrap();
        create_user(&db, "user2", "hash2", Role::User)
            .await
            .unwrap();

        let users = list_all(&db).await.unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].username, "user1");
        assert_eq!(users[1].username, "user2");
    }

    #[tokio::test]
    async fn test_count() {
        let db = setup_db().await;

        assert_eq!(count(&db).await.unwrap(), 0);

        create_user(&db, "user1", "hash1", Role::User)
            .await
            .unwrap();
        assert_eq!(count(&db).await.unwrap(), 1);

        create_user(&db, "user2", "hash2", Role::User)
            .await
            .unwrap();
        assert_eq!(count(&db).await.unwrap(), 2);
    }
}
