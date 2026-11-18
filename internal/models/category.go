package models

import (
	"context"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// Category groups a user's feeds.
type Category struct {
	ID        int64
	UserID    int64
	Name      string
	CreatedAt time.Time
}

const categoryColumns = "id, user_id, name, created_at"

func scanCategory(row scanner) (Category, error) {
	var c Category
	err := row.Scan(&c.ID, &c.UserID, &c.Name, db.TimeDest(&c.CreatedAt))
	return c, err
}

// CreateCategory inserts a category; a duplicate name is ErrCategoryExists.
func CreateCategory(ctx context.Context, e db.Executor, userID int64, name string) (*Category, error) {
	row := e.QueryRowContext(ctx,
		`INSERT INTO category (user_id, name) VALUES ($1, $2) RETURNING `+categoryColumns, userID, name)
	category, err := scanCategory(row)
	if db.IsUniqueViolation(err) {
		return nil, ErrCategoryExists
	}
	if err != nil {
		return nil, err
	}
	return &category, nil
}

func findCategory(ctx context.Context, e db.Executor, where string, args ...any) (*Category, error) {
	row := e.QueryRowContext(ctx, `SELECT `+categoryColumns+` FROM category WHERE `+where, args...)
	category, err := scanCategory(row)
	return orNil(&category, err)
}

// FindCategoryByID looks a category up by id.
func FindCategoryByID(ctx context.Context, e db.Executor, id int64) (*Category, error) {
	return findCategory(ctx, e, "id = $1", id)
}

// FindCategoryByIDAndUser looks a category up by id, scoped to its owner.
func FindCategoryByIDAndUser(ctx context.Context, e db.Executor, id, userID int64) (*Category, error) {
	return findCategory(ctx, e, "id = $1 AND user_id = $2", id, userID)
}

// FindCategoryByNameAndUser looks a category up by name, scoped to its owner.
func FindCategoryByNameAndUser(ctx context.Context, e db.Executor, name string, userID int64) (*Category, error) {
	return findCategory(ctx, e, "name = $1 AND user_id = $2", name, userID)
}

// ListCategoriesByUser returns a user's categories ordered by name.
func ListCategoriesByUser(ctx context.Context, e db.Executor, userID int64) ([]Category, error) {
	rows, err := e.QueryContext(ctx,
		`SELECT `+categoryColumns+` FROM category WHERE user_id = $1 ORDER BY name ASC`, userID)
	return collect(rows, err, scanCategory)
}

// RenameCategory changes a category's name; no matching row is
// ErrCategoryNotFound and a duplicate name is ErrCategoryExists.
func RenameCategory(ctx context.Context, e db.Executor, id, userID int64, newName string) (*Category, error) {
	row := e.QueryRowContext(ctx,
		`UPDATE category SET name = $1 WHERE id = $2 AND user_id = $3 RETURNING `+categoryColumns,
		newName, id, userID)
	category, err := scanCategory(row)
	if db.IsUniqueViolation(err) {
		return nil, ErrCategoryExists
	}
	found, err := orNil(&category, err)
	if err != nil {
		return nil, err
	}
	if found == nil {
		return nil, ErrCategoryNotFound
	}
	return found, nil
}

// DeleteCategory removes a category and, by cascade, its feeds.
func DeleteCategory(ctx context.Context, e db.Executor, id, userID int64) error {
	affected, err := rowsAffected(e.ExecContext(ctx, "DELETE FROM category WHERE id = $1 AND user_id = $2", id, userID))
	if err != nil {
		return err
	}
	if affected == 0 {
		return ErrCategoryNotFound
	}
	return nil
}
