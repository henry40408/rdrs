use maud::{Markup, html};

use super::{layout, macros};
use crate::handlers::pages::{CategoriesTemplate, CategoryRowView};

pub(crate) fn page(page: &CategoriesTemplate) -> Markup {
    let layout = &page.layout;
    let csrf = page.csrf_token.as_str();
    let body = html! {
        div class="app-layout" {
            rdrs-sidebar active="categories" {}
            main class="main-content" {
                (macros::flash_mount(&layout.flash_messages))
                div class="page-content" {
                    h1 { "Categories" }
                    (add_form(csrf))
                    table class="mobile-cards" {
                        thead {
                            tr { th { "Name" } th { "Feeds" } th { "Actions" } }
                        }
                        tbody data-testid="categories-table" {
                            @for category in &page.categories {
                                (category_row(category, csrf))
                            }
                            @if page.categories.is_empty() {
                                tr { td colspan="3" class="empty-state-compact" { "No categories yet — create one using the form above." } }
                            }
                        }
                    }
                }
            }
        }
    };
    layout::app_page(layout, page.title, None, body)
}

fn add_form(csrf: &str) -> Markup {
    html! {
        form method="post" action="/categories" {
            (macros::csrf_field(csrf))
            " "
            div class="form-group" {
                label for="name" { "New Category" }
                " "
                input type="text" id="name" name="name" placeholder="Category name" required maxlength="100" data-testid="category-name-input";
            }
            button type="submit" data-testid="add-category-btn" { "Add Category" }
        }
    }
}

fn category_row(category: &CategoryRowView, csrf: &str) -> Markup {
    let name_input_id = format!("cat-{}-name", category.id);
    html! {
        tr {
            td data-label="Name" {
                form method="post" action={ "/categories/" (category.id) "/rename" } class="cat-rename" {
                    (macros::csrf_field(csrf))
                    " "
                    input id=(name_input_id) type="text" name="name" value=(category.name) maxlength="100" required;
                    " "
                    button type="submit" class="action-link cat-rename-save" { "save" }
                }
            }
            td data-label="Feeds" { (category.feed_count) }
            td class="actions" {
                a href={ "/feeds?category=" (category.id) } { "feeds" }
                " "
                a href={ "/categories/" (category.id) "/entries" } { "entries" }
                " "
                label for=(name_input_id) class="action-link" { "rename" }
                " "
                form method="post" action={ "/categories/" (category.id) "/delete" } class="form-inline" data-confirm="Delete category? This cannot be undone." {
                    (macros::csrf_field(csrf))
                    " "
                    button type="submit" class="action-link action-link-danger" { "delete" }
                }
            }
        }
    }
}
