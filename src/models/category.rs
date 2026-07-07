use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::db::{Db, is_unique_violation};
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_one, query_opt};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Category {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

pub async fn create_category(db: &Db, user_id: i64, name: &str) -> AppResult<Category> {
    query_one!(
        db,
        Category,
        "INSERT INTO category (user_id, name) VALUES ($1, $2) \
         RETURNING id, user_id, name, created_at",
        user_id,
        name
    )
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::CategoryExists
        } else {
            AppError::Database(e)
        }
    })
}

pub async fn find_by_id(db: &Db, id: i64) -> AppResult<Option<Category>> {
    query_opt!(
        db,
        Category,
        "SELECT id, user_id, name, created_at FROM category WHERE id = $1",
        id
    )
    .map_err(AppError::Database)
}

pub async fn find_by_id_and_user(db: &Db, id: i64, user_id: i64) -> AppResult<Option<Category>> {
    query_opt!(
        db,
        Category,
        "SELECT id, user_id, name, created_at FROM category WHERE id = $1 AND user_id = $2",
        id,
        user_id
    )
    .map_err(AppError::Database)
}

pub async fn find_by_name_and_user(
    db: &Db,
    name: &str,
    user_id: i64,
) -> AppResult<Option<Category>> {
    query_opt!(
        db,
        Category,
        "SELECT id, user_id, name, created_at FROM category WHERE name = $1 AND user_id = $2",
        name,
        user_id
    )
    .map_err(AppError::Database)
}

pub async fn list_by_user(db: &Db, user_id: i64) -> AppResult<Vec<Category>> {
    query_all!(
        db,
        Category,
        "SELECT id, user_id, name, created_at FROM category \
         WHERE user_id = $1 ORDER BY name ASC",
        user_id
    )
    .map_err(AppError::Database)
}

pub async fn update_name(db: &Db, id: i64, user_id: i64, new_name: &str) -> AppResult<Category> {
    // `RETURNING` with `fetch_optional` folds the "0 rows matched" case into a
    // `None`, so no separate re-select is needed.
    match query_opt!(
        db,
        Category,
        "UPDATE category SET name = $1 WHERE id = $2 AND user_id = $3 \
         RETURNING id, user_id, name, created_at",
        new_name,
        id,
        user_id
    ) {
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err(AppError::CategoryNotFound),
        Err(e) if is_unique_violation(&e) => Err(AppError::CategoryExists),
        Err(e) => Err(AppError::Database(e)),
    }
}

pub async fn delete_category(db: &Db, id: i64, user_id: i64) -> AppResult<()> {
    let rows = db_execute!(
        db,
        "DELETE FROM category WHERE id = $1 AND user_id = $2",
        id,
        user_id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::CategoryNotFound);
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

    async fn create_test_user(db: &Db, username: &str) -> i64 {
        user::create_user(db, username, "hash123", Role::User)
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn test_create_and_find_category() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;

        let category = create_category(&db, user_id, "Books").await.unwrap();
        assert_eq!(category.name, "Books");
        assert_eq!(category.user_id, user_id);

        let found = find_by_id(&db, category.id).await.unwrap().unwrap();
        assert_eq!(found.name, "Books");

        let found_by_user = find_by_id_and_user(&db, category.id, user_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found_by_user.name, "Books");
    }

    #[tokio::test]
    async fn test_duplicate_category_name() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;

        create_category(&db, user_id, "Books").await.unwrap();
        let result = create_category(&db, user_id, "Books").await;
        assert!(matches!(result, Err(AppError::CategoryExists)));
    }

    #[tokio::test]
    async fn test_same_name_different_users() {
        let db = setup_db().await;
        let user1 = create_test_user(&db, "user1").await;
        let user2 = create_test_user(&db, "user2").await;

        create_category(&db, user1, "Books").await.unwrap();
        let result = create_category(&db, user2, "Books").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_list_by_user_ordered() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;

        create_category(&db, user_id, "Zebra").await.unwrap();
        create_category(&db, user_id, "Apple").await.unwrap();
        create_category(&db, user_id, "Mango").await.unwrap();

        let categories = list_by_user(&db, user_id).await.unwrap();
        assert_eq!(categories.len(), 3);
        assert_eq!(categories[0].name, "Apple");
        assert_eq!(categories[1].name, "Mango");
        assert_eq!(categories[2].name, "Zebra");
    }

    #[tokio::test]
    async fn test_update_name() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;

        let category = create_category(&db, user_id, "Books").await.unwrap();
        let updated = update_name(&db, category.id, user_id, "Novels")
            .await
            .unwrap();
        assert_eq!(updated.name, "Novels");
    }

    #[tokio::test]
    async fn test_update_name_conflict() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;

        create_category(&db, user_id, "Books").await.unwrap();
        let movies = create_category(&db, user_id, "Movies").await.unwrap();

        let result = update_name(&db, movies.id, user_id, "Books").await;
        assert!(matches!(result, Err(AppError::CategoryExists)));
    }

    #[tokio::test]
    async fn test_delete_category() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;

        let category = create_category(&db, user_id, "Books").await.unwrap();
        delete_category(&db, category.id, user_id).await.unwrap();

        assert!(find_by_id(&db, category.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_ownership_check() {
        let db = setup_db().await;
        let user1 = create_test_user(&db, "user1").await;
        let user2 = create_test_user(&db, "user2").await;

        let category = create_category(&db, user1, "Books").await.unwrap();

        // user2 cannot access user1's category
        assert!(
            find_by_id_and_user(&db, category.id, user2)
                .await
                .unwrap()
                .is_none()
        );

        // user2 cannot update user1's category
        let result = update_name(&db, category.id, user2, "Novels").await;
        assert!(matches!(result, Err(AppError::CategoryNotFound)));

        // user2 cannot delete user1's category
        let result = delete_category(&db, category.id, user2).await;
        assert!(matches!(result, Err(AppError::CategoryNotFound)));
    }
}
