use axum::{
    extract::{Query, State},
    Json,
};
use serde::Serialize;

use crate::error::AppResult;
use crate::handlers::pages::{resolve_statistics_period, StatisticsQuery};
use crate::middleware::AuthUser;
use crate::models::statistics;
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct OverviewDto {
    pub total_entries: i64,
    pub read_entries: i64,
    pub unread_entries: i64,
    pub starred_entries: i64,
    pub summaries: i64,
    pub read_rate: f64,
}

#[derive(Debug, Serialize)]
pub struct DailyReadDto {
    pub date: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct CategoryCountDto {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct FeedCountDto {
    pub title: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct AdminStatsDto {
    pub total_users: i64,
    pub total_feeds: i64,
    pub total_entries: i64,
    pub read_entries: i64,
    pub read_rate: f64,
}

#[derive(Debug, Serialize)]
pub struct StatisticsResponse {
    pub active_period: String,
    pub custom_from: String,
    pub custom_to: String,
    pub overview: OverviewDto,
    pub daily_read_counts: Vec<DailyReadDto>,
    pub categories: Vec<CategoryCountDto>,
    pub top_feeds: Vec<FeedCountDto>,
    pub admin: Option<AdminStatsDto>,
}

/// Returns the statistics payload for the period query, mirroring the data
/// the SSR statistics page used to compute. The CSR page module fetches this
/// after mount and renders it.
pub async fn get_statistics(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Query(query): Query<StatisticsQuery>,
) -> AppResult<Json<StatisticsResponse>> {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };
    let show_admin_stats = is_admin && !is_masquerading;

    let (from, to, active_period) = resolve_statistics_period(&query);

    let chart_from = if active_period == "all" {
        let today = chrono::Utc::now().date_naive();
        (today - chrono::Duration::days(90)).to_string()
    } else {
        from.clone()
    };

    let user_id = auth_user.user.id;
    let from_c = from.clone();
    let to_c = to.clone();
    let chart_from_c = chart_from.clone();

    let (overview, daily, cats, feeds, admin_counts, admin_entry_stats) = state
        .db
        .read_user(move |c| {
            let overview =
                statistics::get_personal_overview(c, user_id, &from_c, &to_c).unwrap_or_default();
            let daily = statistics::get_daily_read_counts(c, user_id, &chart_from_c, &to_c)
                .unwrap_or_default();
            let cats =
                statistics::get_entries_by_category(c, user_id, &from_c, &to_c).unwrap_or_default();
            let feeds =
                statistics::get_top_feeds(c, user_id, &from_c, &to_c, 10).unwrap_or_default();

            let admin_counts = if show_admin_stats {
                statistics::get_admin_counts(c).ok()
            } else {
                None
            };
            let admin_entry_stats = if show_admin_stats {
                statistics::get_admin_entry_stats(c, &from_c, &to_c).ok()
            } else {
                None
            };

            Ok::<_, crate::error::AppError>((
                overview,
                daily,
                cats,
                feeds,
                admin_counts,
                admin_entry_stats,
            ))
        })
        .await??;

    let (custom_from, custom_to) = if active_period == "custom" {
        (query.from.unwrap_or_default(), query.to.unwrap_or_default())
    } else {
        (String::new(), String::new())
    };

    let admin = match (admin_counts, admin_entry_stats) {
        (Some(c), Some(e)) => Some(AdminStatsDto {
            total_users: c.total_users,
            total_feeds: c.total_feeds,
            total_entries: e.total_entries,
            read_entries: e.read_entries,
            read_rate: e.read_rate(),
        }),
        _ => None,
    };

    Ok(Json(StatisticsResponse {
        active_period,
        custom_from,
        custom_to,
        overview: OverviewDto {
            total_entries: overview.total_entries,
            read_entries: overview.read_entries,
            unread_entries: overview.unread_entries(),
            starred_entries: overview.starred_entries,
            summaries: overview.summaries,
            read_rate: overview.read_rate(),
        },
        daily_read_counts: daily
            .into_iter()
            .map(|d| DailyReadDto {
                date: d.date.format("%Y-%m-%d").to_string(),
                count: d.count,
            })
            .collect(),
        categories: cats
            .into_iter()
            .map(|c| CategoryCountDto {
                name: c.name,
                count: c.count,
            })
            .collect(),
        top_feeds: feeds
            .into_iter()
            .map(|f| FeedCountDto {
                title: f.title,
                count: f.count,
            })
            .collect(),
        admin,
    }))
}
