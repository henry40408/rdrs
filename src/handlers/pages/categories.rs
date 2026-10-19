use askama::Template;
use axum::extract::State;

use super::{AppLayoutContext, build_app_layout};
use crate::AppState;
use crate::error::AppError;
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::Flash;

/// One row of the SSR `/categories` table.
pub struct CategoryRowView {
    pub id: i64,
    pub name: String,
    pub feed_count: i64,
}

/// Per-route template for `/categories`.
#[derive(Template)]
#[template(path = "categories.html")]
pub struct CategoriesTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
    pub categories: Vec<CategoryRowView>,
}

/// `GET /categories`.
pub async fn categories_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, CategoriesTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_id = auth_user.user.id;

    let categories = async {
        let cats = crate::models::category::list_by_user(&state.db, user_id).await?;
        let feeds = crate::models::feed::list_by_user(&state.db, user_id).await?;
        let mut counts: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
        for f in &feeds {
            *counts.entry(f.category_id).or_insert(0) += 1;
        }
        Ok::<_, AppError>(
            cats.into_iter()
                .map(|c| CategoryRowView {
                    feed_count: *counts.get(&c.id).unwrap_or(&0),
                    id: c.id,
                    name: c.name,
                })
                .collect::<Vec<_>>(),
        )
    }
    .await
    .ok()
    .unwrap_or_default();

    (
        flash,
        CategoriesTemplate {
            title: "Categories",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
            categories,
        },
    )
}
