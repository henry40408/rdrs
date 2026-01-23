use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::models::category::{self, Category};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateCategoryRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCategoryRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct CategoryResponse {
    pub id: i64,
    pub name: String,
    pub created_at: String,
}

impl From<Category> for CategoryResponse {
    fn from(cat: Category) -> Self {
        CategoryResponse {
            id: cat.id,
            name: cat.name,
            created_at: cat.created_at.to_rfc3339(),
        }
    }
}

pub async fn list_categories(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<Vec<CategoryResponse>>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let categories = category::list_by_user(&conn, auth_user.user.id)?;
    let response: Vec<CategoryResponse> = categories.into_iter().map(Into::into).collect();

    Ok(Json(response))
}

pub async fn create_category(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<CreateCategoryRequest>,
) -> AppResult<(StatusCode, Json<CategoryResponse>)> {
    let name = req.name.trim();

    if name.is_empty() {
        return Err(AppError::Validation("Category name cannot be empty".to_string()));
    }

    if name.len() > 100 {
        return Err(AppError::Validation(
            "Category name must be 100 characters or less".to_string(),
        ));
    }

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let cat = category::create_category(&conn, auth_user.user.id, name)?;

    Ok((StatusCode::CREATED, Json(cat.into())))
}

pub async fn get_category(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<CategoryResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let cat = category::find_by_id_and_user(&conn, id, auth_user.user.id)?
        .ok_or(AppError::CategoryNotFound)?;

    Ok(Json(cat.into()))
}

pub async fn update_category(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<UpdateCategoryRequest>,
) -> AppResult<Json<CategoryResponse>> {
    let name = req.name.trim();

    if name.is_empty() {
        return Err(AppError::Validation("Category name cannot be empty".to_string()));
    }

    if name.len() > 100 {
        return Err(AppError::Validation(
            "Category name must be 100 characters or less".to_string(),
        ));
    }

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let cat = category::update_name(&conn, id, auth_user.user.id, name)?;

    Ok(Json(cat.into()))
}

pub async fn delete_category(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    category::delete_category(&conn, id, auth_user.user.id)?;

    Ok(StatusCode::NO_CONTENT)
}
