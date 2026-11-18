package views

import "strconv"

// CategoryRow is one row of the /categories table.
type CategoryRow struct {
	ID        int64
	Name      string
	FeedCount int64
}

// CategoriesPage is the data of GET /categories.
type CategoriesPage struct {
	Layout     *Layout
	CSRFToken  string
	Categories []CategoryRow
}

// Categories renders the category management page.
func Categories(page CategoriesPage) string {
	csrf := page.CSRFToken
	rows := each(page.Categories, func(category CategoryRow) node { return categoryRow(category, csrf) })
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "categories"))),
		el("main", attrs(class("main-content")),
			flashMount(page.Layout.FlashMessages),
			el("div", attrs(class("page-content")),
				el("h1", nil, text("Categories")),
				categoryAddForm(csrf),
				el("table", attrs(class("mobile-cards")),
					el("thead", nil, el("tr", nil,
						el("th", nil, text("Name")), el("th", nil, text("Feeds")), el("th", nil, text("Actions")))),
					el("tbody", attrs(a("data-testid", "categories-table")),
						rows,
						show(len(page.Categories) == 0, el("tr", nil,
							el("td", attrs(a("colspan", "3"), class("empty-state-compact")),
								text("No categories yet — create one using the form above.")))))))))
	return string(appPage(page.Layout, "Categories", "", body))
}

func categoryAddForm(csrf string) node {
	return el("form", attrs(a("method", "post"), a("action", "/categories")),
		csrfField(csrf),
		text(" "),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "name")), text("New Category")),
			text(" "),
			void("input", a("type", "text"), a("id", "name"), a("name", "name"), a("placeholder", "Category name"),
				flag("required"), a("maxlength", "100"), a("data-testid", "category-name-input"))),
		el("button", attrs(a("type", "submit"), a("data-testid", "add-category-btn")), text("Add Category")))
}

func categoryRow(category CategoryRow, csrf string) node {
	id := strconv.FormatInt(category.ID, 10)
	nameInputID := "cat-" + id + "-name"
	return el("tr", nil,
		el("td", attrs(a("data-label", "Name")),
			el("form", attrs(a("method", "post"), a("action", "/categories/"+id+"/rename"), class("cat-rename")),
				csrfField(csrf),
				text(" "),
				void("input", a("id", nameInputID), a("type", "text"), a("name", "name"), a("value", category.Name),
					a("maxlength", "100"), flag("required")),
				text(" "),
				el("button", attrs(a("type", "submit"), class("action-link", "cat-rename-save")), text("save")))),
		el("td", attrs(a("data-label", "Feeds")), num(category.FeedCount)),
		el("td", attrs(class("actions")),
			el("a", attrs(a("href", "/feeds?category="+id)), text("feeds")),
			text(" "),
			el("a", attrs(a("href", "/categories/"+id+"/entries")), text("entries")),
			text(" "),
			el("label", attrs(a("for", nameInputID), class("action-link")), text("rename")),
			text(" "),
			el("form", attrs(a("method", "post"), a("action", "/categories/"+id+"/delete"), class("form-inline"),
				a("data-confirm", "Delete category? This cannot be undone.")),
				csrfField(csrf),
				text(" "),
				el("button", attrs(a("type", "submit"), class("action-link", "action-link-danger")), text("delete")))))
}
