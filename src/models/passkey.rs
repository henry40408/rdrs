use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_one, query_opt};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Passkey {
    pub id: i64,
    pub user_id: i64,
    #[serde(skip_serializing)]
    pub credential_id: Vec<u8>,
    #[serde(skip_serializing)]
    pub public_key: Vec<u8>,
    pub counter: i64,
    pub name: String,
    pub transports: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

pub async fn create_passkey(
    db: &Db,
    user_id: i64,
    credential_id: &[u8],
    public_key: &[u8],
    counter: i64,
    name: &str,
    transports: Option<&str>,
) -> AppResult<Passkey> {
    query_one!(
        db,
        Passkey,
        "INSERT INTO passkey (user_id, credential_id, public_key, counter, name, transports) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, user_id, credential_id, public_key, counter, name, transports, created_at, last_used_at",
        user_id,
        credential_id,
        public_key,
        counter,
        name,
        transports
    )
    .map_err(AppError::Database)
}

pub async fn find_by_id(db: &Db, id: i64) -> AppResult<Option<Passkey>> {
    query_opt!(
        db,
        Passkey,
        "SELECT id, user_id, credential_id, public_key, counter, name, transports, created_at, last_used_at FROM passkey WHERE id = $1",
        id
    )
    .map_err(AppError::Database)
}

pub async fn find_by_credential_id(db: &Db, credential_id: &[u8]) -> AppResult<Option<Passkey>> {
    query_opt!(
        db,
        Passkey,
        "SELECT id, user_id, credential_id, public_key, counter, name, transports, created_at, last_used_at FROM passkey WHERE credential_id = $1",
        credential_id
    )
    .map_err(AppError::Database)
}

pub async fn list_by_user(db: &Db, user_id: i64) -> AppResult<Vec<Passkey>> {
    query_all!(
        db,
        Passkey,
        "SELECT id, user_id, credential_id, public_key, counter, name, transports, created_at, last_used_at FROM passkey WHERE user_id = $1 ORDER BY created_at DESC",
        user_id
    )
    .map_err(AppError::Database)
}

// There is deliberately no "every passkey on the instance" query. The one
// caller that wanted it was the sign-in challenge, which used the result to
// fill `allowCredentials` and thereby handed every account's credential ID to
// any unauthenticated caller. That flow is discoverable now (see
// `handlers::passkey::start_authentication`) and needs no such read; leaving
// the helper behind would only invite the leak back.

pub async fn update_counter(db: &Db, id: i64, counter: i64) -> AppResult<()> {
    db_execute!(
        db,
        "UPDATE passkey SET counter = $1, last_used_at = $2 WHERE id = $3",
        counter,
        Utc::now(),
        id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

pub async fn rename_passkey(db: &Db, id: i64, user_id: i64, name: &str) -> AppResult<()> {
    let updated = db_execute!(
        db,
        "UPDATE passkey SET name = $1 WHERE id = $2 AND user_id = $3",
        name,
        id,
        user_id
    )
    .map_err(AppError::Database)?;
    if updated == 0 {
        return Err(AppError::PasskeyNotFound);
    }
    Ok(())
}

pub async fn delete_passkey(db: &Db, id: i64, user_id: i64) -> AppResult<()> {
    let deleted = db_execute!(
        db,
        "DELETE FROM passkey WHERE id = $1 AND user_id = $2",
        id,
        user_id
    )
    .map_err(AppError::Database)?;
    if deleted == 0 {
        return Err(AppError::PasskeyNotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::{self, Role};

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_create_and_find_passkey() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let credential_id = vec![1, 2, 3, 4];
        let public_key = vec![5, 6, 7, 8];

        let passkey = create_passkey(
            &db,
            user.id,
            &credential_id,
            &public_key,
            0,
            "My Passkey",
            Some("usb,nfc"),
        )
        .await
        .unwrap();

        assert_eq!(passkey.user_id, user.id);
        assert_eq!(passkey.credential_id, credential_id);
        assert_eq!(passkey.name, "My Passkey");

        let found = find_by_id(&db, passkey.id).await.unwrap().unwrap();
        assert_eq!(found.id, passkey.id);

        let found = find_by_credential_id(&db, &credential_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, passkey.id);
    }

    #[tokio::test]
    async fn test_list_passkeys() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        create_passkey(&db, user.id, &[1], &[1], 0, "Passkey 1", None)
            .await
            .unwrap();
        create_passkey(&db, user.id, &[2], &[2], 0, "Passkey 2", None)
            .await
            .unwrap();

        let passkeys = list_by_user(&db, user.id).await.unwrap();
        assert_eq!(passkeys.len(), 2);
    }

    #[tokio::test]
    async fn test_update_counter() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let passkey = create_passkey(&db, user.id, &[1], &[1], 0, "Passkey", None)
            .await
            .unwrap();
        assert_eq!(passkey.counter, 0);
        assert!(passkey.last_used_at.is_none());

        update_counter(&db, passkey.id, 5).await.unwrap();

        let updated = find_by_id(&db, passkey.id).await.unwrap().unwrap();
        assert_eq!(updated.counter, 5);
        assert!(updated.last_used_at.is_some());
    }

    #[tokio::test]
    async fn test_rename_passkey() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let passkey = create_passkey(&db, user.id, &[1], &[1], 0, "Old Name", None)
            .await
            .unwrap();
        rename_passkey(&db, passkey.id, user.id, "New Name")
            .await
            .unwrap();

        let updated = find_by_id(&db, passkey.id).await.unwrap().unwrap();
        assert_eq!(updated.name, "New Name");
    }

    #[tokio::test]
    async fn test_delete_passkey() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let passkey = create_passkey(&db, user.id, &[1], &[1], 0, "Passkey", None)
            .await
            .unwrap();
        delete_passkey(&db, passkey.id, user.id).await.unwrap();

        let found = find_by_id(&db, passkey.id).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_delete_passkey_wrong_user() {
        let db = setup_db().await;
        let user1 = user::create_user(&db, "user1", "hash", Role::User)
            .await
            .unwrap();
        let user2 = user::create_user(&db, "user2", "hash", Role::User)
            .await
            .unwrap();

        let passkey = create_passkey(&db, user1.id, &[1], &[1], 0, "Passkey", None)
            .await
            .unwrap();

        let result = delete_passkey(&db, passkey.id, user2.id).await;
        assert!(matches!(result, Err(AppError::PasskeyNotFound)));
    }

    #[tokio::test]
    async fn passkeys_are_only_ever_listed_per_user() {
        // Replaces a test for a cross-user `get_all_passkeys`. Each user's
        // listing must show only their own credentials — the sign-in flow no
        // longer has any reason to read another account's, and nothing else
        // ever did.
        let db = setup_db().await;
        let user1 = user::create_user(&db, "user1", "hash", Role::User)
            .await
            .unwrap();
        let user2 = user::create_user(&db, "user2", "hash", Role::User)
            .await
            .unwrap();

        create_passkey(&db, user1.id, &[1], &[1], 0, "User1 Passkey", None)
            .await
            .unwrap();
        create_passkey(&db, user2.id, &[2], &[2], 0, "User2 Passkey", None)
            .await
            .unwrap();

        let mine = list_by_user(&db, user1.id).await.unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].name, "User1 Passkey");
    }

    #[tokio::test]
    async fn test_rename_passkey_wrong_user() {
        let db = setup_db().await;
        let user1 = user::create_user(&db, "user1", "hash", Role::User)
            .await
            .unwrap();
        let user2 = user::create_user(&db, "user2", "hash", Role::User)
            .await
            .unwrap();

        let passkey = create_passkey(&db, user1.id, &[1], &[1], 0, "Passkey", None)
            .await
            .unwrap();

        let result = rename_passkey(&db, passkey.id, user2.id, "New Name").await;
        assert!(matches!(result, Err(AppError::PasskeyNotFound)));
    }

    #[tokio::test]
    async fn test_find_by_credential_id_not_found() {
        let db = setup_db().await;

        let result = find_by_credential_id(&db, &[99, 99, 99]).await.unwrap();
        assert!(result.is_none());
    }
}
