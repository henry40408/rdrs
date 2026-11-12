//! `/statistics`.

use maud::{Markup, html};

use super::layout::{app_page, module_script};
use super::macros;
use crate::handlers::pages::{
    AdminDatabaseStatsView, AdminStatsView, DailyReadView, FeedOpenRateView, StatisticsTemplate,
};

pub(crate) fn statistics(page: &StatisticsTemplate) -> Markup {
    let layout = &page.layout;
    let body = html! {
        div.app-layout {
            rdrs-sidebar active="statistics" {}
            main.main-content {
                (macros::flash_mount(&layout.flash_messages))
                div.page-content {
                    (header(page))
                    (overview_cards(page))
                    (daily_chart(page))
                    div.stats-columns {
                        (breakdown("Entries by Category", page.categories.iter().map(|c| BarRow { label: &c.name, detail: c.count.to_string(), width_percent: c.width_percent })))
                        (breakdown("Top Feeds", page.top_feeds.iter().map(|f| BarRow { label: &f.title, detail: f.count.to_string(), width_percent: f.width_percent })))
                    }
                    @if let Some(since) = &page.tracked_since { (open_rate_section(page, since)) }
                    @if let Some(admin) = &page.admin { (site_section(admin)) }
                    @if let Some(db) = &page.admin_db { (database_section(db)) }
                }
            }
        }
    };
    let script = module_script(layout.git_version, "statistics.js");
    app_page(layout, page.title, Some(script), body)
}

fn period_link(page: &StatisticsTemplate, period: &str, label: &str) -> Markup {
    html! {
        a href={ "/statistics?period=" (period) } class={ "stats-period-btn" @if page.active_period == period { " active" } } { (label) }
    }
}

fn header(page: &StatisticsTemplate) -> Markup {
    html! {
        div.stats-header {
            h1 { "Statistics" }
            form.stats-period method="get" action="/statistics" {
                (period_link(page, "7d", "7d"))
                " "
                (period_link(page, "30d", "30d"))
                " "
                (period_link(page, "90d", "90d"))
                " "
                (period_link(page, "all", "All"))
                " "
                span.stats-period-divider { "|" }
                " "
                input type="hidden" name="period" value="custom";
                " "
                input type="date" name="from" value=(page.custom_from) class="stats-date-input";
                " "
                span.stats-period-dash { "—" }
                " "
                input type="date" name="to" value=(page.custom_to) class="stats-date-input";
                " "
                button type="submit" class="stats-period-btn" { "Apply" }
            }
        }
    }
}

fn stat_card(value: Markup, label: &str) -> Markup {
    html! {
        div.stats-card {
            (value)
            div.stats-card-label { (label) }
        }
    }
}

fn overview_cards(page: &StatisticsTemplate) -> Markup {
    html! {
        div class="stats-cards stats-cards--tiles" {
            (stat_card(html! { div.stats-card-value { (page.total_entries) } }, "Total Entries"))
            (stat_card(html! { div.stats-card-value.stats-card-success { (page.read_entries) } }, "Read"))
            (stat_card(html! { div.stats-card-value.stats-card-warning { (page.unread_entries) } }, "Unread"))
            (stat_card(html! { div.stats-card-value { (page.read_rate_fmt) "%" } }, "Read Rate"))
            (stat_card(html! { div.stats-card-value { (page.starred_entries) } }, "Starred"))
            (stat_card(html! { div.stats-card-value { (page.summaries) } }, "Summaries"))
        }
    }
}

fn daily_chart(page: &StatisticsTemplate) -> Markup {
    html! {
        div.stats-section {
            h2 { "Daily Read Articles" }
            @if page.daily_max == 0 {
                p.muted { "No read activity in this period" }
            } @else {
                div class={ "stats-chart" @if page.daily_read_counts.len() > 10 { " stats-chart--dense" } } {
                    @for day in &page.daily_read_counts { (daily_bar(day)) }
                }
            }
        }
    }
}

fn daily_bar(day: &DailyReadView) -> Markup {
    html! {
        div.stats-bar-col tabindex="0" aria-label={ (day.date_label) ": " (day.count) " read" } {
            div.stats-bar-tip role="tooltip" { (day.date_label) ": " strong { (day.count) } }
            div class={ "stats-bar pct-" (day.height_percent) } {
                @if day.is_max { span.stats-bar-value { (day.count) } }
            }
            div.stats-bar-label { (day.short_label) }
        }
    }
}

/// One line of a bar list: a label, its right-hand figure and the bar width.
struct BarRow<'a> {
    label: &'a str,
    detail: String,
    width_percent: u8,
}

fn bar_row(row: BarRow<'_>) -> Markup {
    html! {
        div.stats-bar-row {
            div.stats-bar-row-header {
                span { (row.label) }
                " "
                span.muted { (row.detail) }
            }
            div.stats-progress {
                div class={ "stats-progress-fill pct-" (row.width_percent) } {}
            }
        }
    }
}

fn breakdown<'a>(heading: &str, rows: impl Iterator<Item = BarRow<'a>>) -> Markup {
    let rows: Vec<BarRow<'a>> = rows.collect();
    html! {
        div.stats-section {
            h2 { (heading) }
            @if rows.is_empty() {
                p.empty-state-compact { "No entries in this period." }
            } @else {
                @for row in rows { (bar_row(row)) }
            }
        }
    }
}

fn open_rate_section(page: &StatisticsTemplate, since: &str) -> Markup {
    html! {
        div.stats-section data-testid="stats-open-rate" {
            h2 { "Feeds by Open Rate" }
            // Lowest first: the top of this list is what to unsubscribe from.
            p.muted.text-xs { "How often a feed's entries actually get opened, lowest first. Tracked since " (since) "; feeds with fewer than " (page.min_tracked_for_rate) " tracked entries are not listed." }
            @if page.open_rate_feeds.is_empty() {
                p.empty-state-compact { "No feed has enough tracked entries yet." }
            } @else {
                @for feed in &page.open_rate_feeds { (bar_row(open_rate_row(feed))) }
            }
        }
    }
}

fn open_rate_row(feed: &FeedOpenRateView) -> BarRow<'_> {
    BarRow {
        label: &feed.title,
        detail: format!("{}% ({}/{})", feed.percent, feed.opened, feed.tracked),
        width_percent: feed.width_percent,
    }
}

fn admin_card(value: Markup, label: Markup) -> Markup {
    html! {
        div.stats-card.stats-card-admin {
            (value)
            div.stats-card-label { (label) }
        }
    }
}

fn site_section(admin: &AdminStatsView) -> Markup {
    html! {
        div.stats-admin-section {
            h2 { "Site-wide Statistics" }
            div.stats-cards {
                (admin_card(html! { div.stats-card-value { (admin.total_users) } }, html! { "Total Users" }))
                (admin_card(html! { div.stats-card-value data-testid="stat-site-entries-total" { (admin.total_entries) } }, html! { "Site Entries · in period" }))
                (admin_card(html! { div.stats-card-value data-testid="stat-site-feeds-total" { (admin.total_feeds) } }, html! { "Total Feeds" }))
                (admin_card(html! { div.stats-card-value { (admin.read_rate_fmt) "%" } }, html! { "Site Read Rate · in period" }))
            }
        }
    }
}

fn database_section(db: &AdminDatabaseStatsView) -> Markup {
    html! {
        div.stats-admin-section {
            h2 { "Database" }
            div class="stats-cards stats-cards--db" {
                (admin_card(html! { div.stats-card-value { (db.size_fmt) } }, html! { "Database Size" }))
                @if let Some(r) = &db.reclaimable {
                    (admin_card(html! { div.stats-card-value { (r.size_fmt) } }, html! { "Reclaimable · " (r.frag_pct) "% of file" }))
                }
                (admin_card(html! { div.stats-card-value data-testid="stat-db-total-entries" { (db.total_entries) } }, html! { "Total Entries" }))
                (admin_card(html! { div.stats-card-value { (db.avg_per_day_fmt) } }, html! { "Avg New / Day · retained" }))
                (admin_card(html! { div.stats-card-value { (db.coverage_fmt) } }, html! { "Coverage" }))
                (admin_card(html! { div.stats-card-value data-testid="stat-db-pruned" { (db.tombstone_count) } }, html! { "Pruned Entries" }))
            }
        }
    }
}
