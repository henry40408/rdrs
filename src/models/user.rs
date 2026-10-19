use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::{Db, Tx, is_unique_violation};
use crate::error::{AppError, AppResult};
use crate::models::{api_token, session};
use crate::{
    db_execute, db_execute_tx, query_all, query_one, query_one_tx, query_opt, query_scalar,
    query_scalar_tx,
};

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

// `role` is TEXT on both backends: decode via `String`; binds pass `role.as_str()`,
// so no `Encode` impl is needed.
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

/// [`create_user`] inside a caller-owned transaction.
pub async fn create_user_tx(
    tx: &mut Tx<'_>,
    username: &str,
    password_hash: &str,
    role: Role,
) -> AppResult<User> {
    query_one_tx!(
        tx,
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

/// Set a new password and revoke every credential issued under the old one, as
/// one transaction: a failed revocation must not leave the new password live
/// beside old sessions and API tokens.
pub async fn change_password(db: &Db, user_id: i64, new_password_hash: &str) -> AppResult<()> {
    let mut tx = db.begin().await?;
    change_password_tx(&mut tx, user_id, new_password_hash).await?;
    tx.commit().await?;
    Ok(())
}

/// [`change_password`] inside a caller-owned transaction.
pub async fn change_password_tx(
    tx: &mut Tx<'_>,
    user_id: i64,
    new_password_hash: &str,
) -> AppResult<()> {
    let rows = db_execute_tx!(
        tx,
        "UPDATE \"user\" SET password_hash = $1 WHERE id = $2",
        new_password_hash,
        user_id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    revoke_credentials_tx(tx, user_id).await
}

/// Sessions plus API tokens; tokens bypass `session`, so both must go.
async fn revoke_credentials_tx(tx: &mut Tx<'_>, user_id: i64) -> AppResult<()> {
    session::delete_user_sessions_tx(tx, user_id).await?;
    api_token::delete_user_tokens_tx(tx, user_id).await?;
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

/// Disable the account and revoke its sessions and API tokens in one
/// transaction, so a disabled user is never left holding a live credential.
pub async fn disable_user(db: &Db, user_id: i64) -> AppResult<()> {
    let mut tx = db.begin().await?;
    let rows = db_execute_tx!(
        &mut tx,
        "UPDATE \"user\" SET disabled_at = $1 WHERE id = $2",
        Utc::now(),
        user_id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    revoke_credentials_tx(&mut tx, user_id).await?;
    tx.commit().await?;
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

pub async fn count_tx(tx: &mut Tx<'_>) -> AppResult<i64> {
    query_scalar_tx!(tx, i64, "SELECT COUNT(*) FROM \"user\"").map_err(AppError::Database)
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

    /// A user holding one session and one API token.
    async fn user_with_credentials(db: &Db) -> User {
        let user = create_user(db, "holder", "old-hash", Role::User)
            .await
            .unwrap();
        session::create_session(db, user.id, "ua", "127.0.0.1")
            .await
            .unwrap();
        api_token::create_api_token(db, user.id, "greader", "label", "ua", "127.0.0.1")
            .await
            .unwrap();
        user
    }

    async fn credential_counts(db: &Db, user_id: i64) -> (usize, usize) {
        let sessions = session::list_user_sessions(db, user_id).await.unwrap();
        let tokens = api_token::list_user_tokens(db, user_id).await.unwrap();
        (sessions.len(), tokens.len())
    }

    #[tokio::test]
    async fn change_password_revokes_sessions_and_api_tokens() {
        let db = setup_db().await;
        let user = user_with_credentials(&db).await;

        change_password(&db, user.id, "new-hash").await.unwrap();

        let after = find_by_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(after.password_hash, "new-hash");
        assert_eq!(credential_counts(&db, user.id).await, (0, 0));
    }

    #[tokio::test]
    async fn change_password_rolls_back_when_revocation_fails() {
        let db = setup_db().await;
        let user = user_with_credentials(&db).await;
        db_execute!(&db, "DROP TABLE api_token").unwrap();

        assert!(change_password(&db, user.id, "new-hash").await.is_err());

        let after = find_by_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(after.password_hash, "old-hash", "password must not change");
        let sessions = session::list_user_sessions(&db, user.id).await.unwrap();
        assert_eq!(sessions.len(), 1, "session deletion must roll back too");
    }

    #[tokio::test]
    async fn disable_user_revokes_sessions_and_api_tokens() {
        let db = setup_db().await;
        let user = user_with_credentials(&db).await;

        disable_user(&db, user.id).await.unwrap();

        let after = find_by_id(&db, user.id).await.unwrap().unwrap();
        assert!(after.is_disabled());
        assert_eq!(credential_counts(&db, user.id).await, (0, 0));
    }

    #[tokio::test]
    async fn disable_user_rolls_back_when_revocation_fails() {
        let db = setup_db().await;
        let user = user_with_credentials(&db).await;
        db_execute!(&db, "DROP TABLE api_token").unwrap();

        assert!(disable_user(&db, user.id).await.is_err());

        let after = find_by_id(&db, user.id).await.unwrap().unwrap();
        assert!(!after.is_disabled(), "account must stay enabled");
        let sessions = session::list_user_sessions(&db, user.id).await.unwrap();
        assert_eq!(sessions.len(), 1, "session deletion must roll back too");
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
