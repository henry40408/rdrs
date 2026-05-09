use axum::{
    extract::{Form, Path, State},
    response::IntoResponse,
};
use serde::Deserialize;

use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::middleware::flash::FlashRedirect;
use crate::models::category;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CategoryNameForm {
    pub name: String,
}

pub async fn create_category_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Form(req): Form<CategoryNameForm>,
) -> impl IntoResponse {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return FlashRedirect::error("/categories", "Category name cannot be empty");
    }
    if name.len() > 100 {
        return FlashRedirect::error("/categories", "Category name is too long (max 100)");
    }
    let user_id = auth_user.user.id;
    let result = state
        .db
        .user(move |conn| category::create_category(conn, user_id, &name))
        .await;
    match result {
        Ok(Ok(_)) => FlashRedirect::success("/categories", "Category created."),
        Ok(Err(AppError::CategoryExists)) => {
            FlashRedirect::error("/categories", "Category already exists.")
        }
        Ok(Err(AppError::Validation(msg))) => FlashRedirect::error("/categories", msg),
        _ => FlashRedirect::error("/categories", "Failed to create category."),
    }
}

pub async fn rename_category_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
    Form(req): Form<CategoryNameForm>,
) -> impl IntoResponse {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return FlashRedirect::error("/categories", "Category name cannot be empty");
    }
    if name.len() > 100 {
        return FlashRedirect::error("/categories", "Category name is too long (max 100)");
    }
    let user_id = auth_user.user.id;
    let result = state
        .db
        .user(move |conn| category::update_name(conn, id, user_id, &name))
        .await;
    match result {
        Ok(Ok(_)) => FlashRedirect::success("/categories", "Category renamed."),
        Ok(Err(AppError::CategoryNotFound)) => {
            FlashRedirect::error("/categories", "Category not found.")
        }
        Ok(Err(AppError::CategoryExists)) => {
            FlashRedirect::error("/categories", "Category name already in use.")
        }
        Ok(Err(AppError::Validation(msg))) => FlashRedirect::error("/categories", msg),
        _ => FlashRedirect::error("/categories", "Failed to rename category."),
    }
}

pub async fn delete_category_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let user_id = auth_user.user.id;
    let result = state
        .db
        .user(move |conn| category::delete_category(conn, id, user_id))
        .await;
    match result {
        Ok(Ok(_)) => FlashRedirect::success("/categories", "Category deleted."),
        Ok(Err(AppError::CategoryNotFound)) => {
            FlashRedirect::error("/categories", "Category not found.")
        }
        Ok(Err(AppError::Validation(msg))) => FlashRedirect::error("/categories", msg),
        _ => FlashRedirect::error("/categories", "Failed to delete category."),
    }
}
