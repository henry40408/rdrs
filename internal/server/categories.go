package server

import (
	"errors"
	"net/http"
	"strings"

	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/views"
)

// categoryRoutes are the category pages and their no-JS form actions.
func (a *App) categoryRoutes() []Route {
	return []Route{
		{"GET", "/categories", a.categoriesPage},
		{"POST", "/categories", a.createCategoryForm},
		{"POST", "/categories/{id}/rename", a.renameCategoryForm},
		{"POST", "/categories/{id}/delete", a.deleteCategoryForm},
	}
}

const categoriesLocation = "/categories"

func categoryFlash(w http.ResponseWriter, message flash.Message) {
	flash.Redirect(w, categoriesLocation, message)
}

// categoriesPage is GET /categories.
func (a *App) categoriesPage(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	writeHTML(w, http.StatusOK, views.Categories(views.CategoriesPage{
		Layout:     &layout,
		CSRFToken:  principal.CSRFToken,
		Categories: a.categoryRows(r, principal.User.ID),
	}))
}

// categoryRows lists the categories with their feed counts; a failing query
// shows an empty table rather than an error page.
func (a *App) categoryRows(r *http.Request, userID int64) []views.CategoryRow {
	categories, err := models.ListCategoriesByUser(r.Context(), a.DB, userID)
	if err != nil {
		return nil
	}
	feeds, err := models.ListFeedsByUser(r.Context(), a.DB, userID)
	if err != nil {
		return nil
	}
	counts := map[int64]int64{}
	for _, feed := range feeds {
		counts[feed.CategoryID]++
	}
	rows := make([]views.CategoryRow, 0, len(categories))
	for _, category := range categories {
		rows = append(rows, views.CategoryRow{ID: category.ID, Name: category.Name, FeedCount: counts[category.ID]})
	}
	return rows
}

// categoryNameOrFlash trims and validates the submitted name, answering the
// refusal itself. The length limit counts bytes, as the Rust check did.
func categoryNameOrFlash(w http.ResponseWriter, name string) (string, bool) {
	name = strings.TrimSpace(name)
	switch {
	case name == "":
		categoryFlash(w, flash.NewError("Category name cannot be empty"))
		return "", false
	case len(name) > 100:
		categoryFlash(w, flash.NewError("Category name is too long (max 100)"))
		return "", false
	}
	return name, true
}

// failureMessage is the flash text of a failed category write: known errors
// get their own wording, anything else the generic fallback.
func failureMessage(err error, known map[error]string, fallback string) string {
	var validation *models.ValidationError
	if errors.As(err, &validation) {
		return validation.Message
	}
	for target, message := range known {
		if errors.Is(err, target) {
			return message
		}
	}
	return fallback
}

// createCategoryForm is POST /categories.
func (a *App) createCategoryForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "name") {
		return
	}
	name, ok := categoryNameOrFlash(w, form.Get("name"))
	if !ok {
		return
	}
	userID := principal.User.ID
	if _, err := models.CreateCategory(r.Context(), a.DB, userID, name); err != nil {
		categoryFlash(w, flash.NewError(failureMessage(err,
			map[error]string{models.ErrCategoryExists: "Category already exists."}, "Failed to create category.")))
		return
	}
	a.Sidebar.Bust(userID)
	categoryFlash(w, flash.NewSuccess("Category created."))
}

// renameCategoryForm is POST /categories/{id}/rename.
func (a *App) renameCategoryForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "name") {
		return
	}
	name, ok := categoryNameOrFlash(w, form.Get("name"))
	if !ok {
		return
	}
	userID := principal.User.ID
	if _, err := models.RenameCategory(r.Context(), a.DB, id, userID, name); err != nil {
		categoryFlash(w, flash.NewError(failureMessage(err, map[error]string{
			models.ErrCategoryNotFound: "Category not found.",
			models.ErrCategoryExists:   "Category name already in use.",
		}, "Failed to rename category.")))
		return
	}
	a.Sidebar.Bust(userID)
	categoryFlash(w, flash.NewSuccess("Category renamed."))
}

// deleteCategoryForm is POST /categories/{id}/delete.
func (a *App) deleteCategoryForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	userID := principal.User.ID
	if err := models.DeleteCategory(r.Context(), a.DB, id, userID); err != nil {
		categoryFlash(w, flash.NewError(failureMessage(err,
			map[error]string{models.ErrCategoryNotFound: "Category not found."}, "Failed to delete category.")))
		return
	}
	a.Sidebar.Bust(userID)
	categoryFlash(w, flash.NewSuccess("Category deleted."))
}
