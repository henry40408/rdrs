use axum::{
    extract::{Form, Path, State},
    response::IntoResponse,
};
use serde::Deserialize;

use crate::AppState;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::middleware::flash::FlashRedirect;
use crate::models::category;

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
    let result = category::create_category(&state.db, user_id, &name).await;
    match result {
        Ok(_) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/categories", "Category created.")
        }
        Err(AppError::CategoryExists) => {
            FlashRedirect::error("/categories", "Category already exists.")
        }
        Err(AppError::Validation(msg)) => FlashRedirect::error("/categories", msg),
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
    let result = category::update_name(&state.db, id, user_id, &name).await;
    match result {
        Ok(_) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/categories", "Category renamed.")
        }
        Err(AppError::CategoryNotFound) => {
            FlashRedirect::error("/categories", "Category not found.")
        }
        Err(AppError::CategoryExists) => {
            FlashRedirect::error("/categories", "Category name already in use.")
        }
        Err(AppError::Validation(msg)) => FlashRedirect::error("/categories", msg),
        _ => FlashRedirect::error("/categories", "Failed to rename category."),
    }
}

pub async fn delete_category_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let user_id = auth_user.user.id;
    let result = category::delete_category(&state.db, id, user_id).await;
    match result {
        Ok(()) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/categories", "Category deleted.")
        }
        Err(AppError::CategoryNotFound) => {
            FlashRedirect::error("/categories", "Category not found.")
        }
        Err(AppError::Validation(msg)) => FlashRedirect::error("/categories", msg),
        _ => FlashRedirect::error("/categories", "Failed to delete category."),
    }
}
