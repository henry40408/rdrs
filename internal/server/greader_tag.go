package server

import (
	"context"
	"net/http"
	"net/url"
	"strconv"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/greader"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// greaderSidebarChanged drops the cached sidebar and tells open pages.
func (a *App) greaderSidebarChanged(userID int64) {
	a.Sidebar.Bust(userID)
	a.Events.EmitSidebar(userID)
}

// greaderTagList is GET /reader/api/0/tag/list.
func (a *App) greaderTagList(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	categories, err := models.ListCategoriesByUser(r.Context(), a.DB, user.user.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	state, folder := ptr("state"), ptr("folder")
	tags := []greader.Tag{
		{ID: greader.ReadingListID, Type: state}, {ID: greader.ReadID, Type: state},
		{ID: greader.StarredID, Type: state}, {ID: greader.KeptUnreadID, Type: state},
	}
	for _, category := range categories {
		tags = append(tags, greader.Tag{
			ID:   greader.Stream{Kind: greader.Label, Name: category.Name}.String(),
			Type: folder, SortID: ptr(sortID(category.ID)),
		})
	}
	writeJSON(w, http.StatusOK, greader.TagList{Tags: tags})
}

// sortID is the 8-digit hex ordering key clients sort by.
func sortID(id int64) string {
	hex := strconv.FormatUint(uint64(id), 16)
	for len(hex) < 8 {
		hex = "0" + hex
	}
	return hex
}

// streamsOf parses the optional a/r stream of an edit-tag form; the last value
// wins, as with a serde form.
func streamsOf(form url.Values, name string) (*greader.Stream, error) {
	values := form[name]
	if len(values) == 0 {
		return nil, nil
	}
	stream, err := greader.ParseStream(values[len(values)-1])
	if err != nil {
		return nil, err
	}
	return &stream, nil
}

// greaderEditTag is POST /reader/api/0/edit-tag.
func (a *App) greaderEditTag(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	affected, err := a.editTag(r.Context(), user.user.ID, form)
	if err != nil {
		writeError(w, err)
		return
	}
	a.greaderSidebarChanged(user.user.ID)
	writeOKWithAffected(w, affected)
}

func (a *App) editTag(ctx context.Context, userID int64, form url.Values) (int64, error) {
	if len(form["i"]) == 0 {
		return 0, ValidationError("No item IDs provided")
	}
	entryIDs, err := entryIDsOf(form["i"])
	if err != nil {
		return 0, appError(err)
	}
	add, err := streamsOf(form, "a")
	if err != nil {
		return 0, appError(err)
	}
	remove, err := streamsOf(form, "r")
	if err != nil {
		return 0, appError(err)
	}
	owned, err := entry.FindByIDsWithFeed(ctx, a.DB, userID, entryIDs)
	if err != nil {
		return 0, DatabaseError(err)
	}
	// All or nothing: one foreign or missing id fails the whole batch.
	if len(owned) != len(entryIDs) {
		return 0, ErrEntryNotFound
	}
	if add != nil && add.Kind == greader.Read {
		affected, err := entry.MarkReadByIDs(ctx, a.DB, userID, entryIDs)
		if err != nil {
			return 0, DatabaseError(err)
		}
		return affected, nil
	}
	return a.applyTagChanges(ctx, userID, entryIDs, add, remove)
}

// applyTagChanges runs the star and unread changes in one transaction.
func (a *App) applyTagChanges(ctx context.Context, userID int64, entryIDs []int64, add, remove *greader.Stream) (int64, error) {
	tx, err := a.DB.Begin(ctx)
	if err != nil {
		return 0, DatabaseError(err)
	}
	defer func() { _ = tx.Rollback() }()
	var affected int64
	for _, change := range tagChanges(add, remove) {
		count, err := change(ctx, tx, userID, entryIDs)
		if err != nil {
			return 0, DatabaseError(err)
		}
		affected += count
	}
	if err := tx.Commit(); err != nil {
		return 0, DatabaseError(err)
	}
	return affected, nil
}

type tagChange func(ctx context.Context, e db.Executor, userID int64, entryIDs []int64) (int64, error)

func tagChanges(add, remove *greader.Stream) []tagChange {
	var changes []tagChange
	if add != nil && add.Kind == greader.Starred {
		changes = append(changes, entry.StarByIDs)
	}
	if add != nil && add.Kind == greader.KeptUnread {
		changes = append(changes, entry.MarkUnreadByIDs)
	}
	if remove != nil && remove.Kind == greader.Read {
		changes = append(changes, entry.MarkUnreadByIDs)
	}
	if remove != nil && remove.Kind == greader.Starred {
		changes = append(changes, entry.UnstarByIDs)
	}
	return changes
}

// greaderMarkAllAsRead is POST /reader/api/0/mark-all-as-read.
func (a *App) greaderMarkAllAsRead(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "s") {
		return
	}
	stream, ok := parseStreamParam(w, form.Get("s"))
	if !ok {
		return
	}
	limits, err := markAllLimits(form, time.Now())
	if err != nil {
		writeError(w, err)
		return
	}
	affected, err := a.markAllRead(r.Context(), user.user.ID, stream, limits)
	if err != nil {
		writeError(w, err)
		return
	}
	a.greaderSidebarChanged(user.user.ID)
	writeOKWithAffected(w, affected)
}

// markAllLimits reads the snapshot and ts fields. A malformed snapshot is
// refused: ignoring it would mark everything, the opposite of its purpose.
func markAllLimits(form url.Values, now time.Time) (entry.MarkAllRead, error) {
	var limits entry.MarkAllRead
	if raw := form.Get("snapshot"); form.Has("snapshot") {
		snapshot, err := time.Parse("2006-01-02 15:04:05", raw)
		if err != nil {
			return limits, ValidationError("snapshot must be UTC YYYY-MM-DD HH:MM:SS")
		}
		limits.StoredBy = &snapshot
	}
	// An unparsable ts is ignored; a ts in the future means "nothing older".
	timestampUsec, err := strconv.ParseInt(form.Get("ts"), 10, 64)
	if err != nil {
		return limits, nil
	}
	if age := now.Unix() - timestampUsec/1_000_000; age > 0 {
		limits.OlderThanDays = ptr(age / 86400)
	}
	return limits, nil
}

func (a *App) markAllRead(ctx context.Context, userID int64, stream greader.Stream, limits entry.MarkAllRead) (int64, error) {
	var affected int64
	var err error
	switch stream.Kind {
	case greader.ReadingList:
		affected, err = entry.MarkAllReadByUser(ctx, a.DB, userID, limits)
	case greader.Feed:
		found, lookupErr := models.FindFeedByURLForUser(ctx, a.DB, stream.Name, userID)
		if lookupErr != nil {
			return 0, DatabaseError(lookupErr)
		}
		if found == nil {
			return 0, ErrFeedNotFound
		}
		affected, err = entry.MarkAllReadByFeed(ctx, a.DB, found.ID, limits)
	case greader.Label:
		found, lookupErr := models.FindCategoryByNameAndUser(ctx, a.DB, stream.Name, userID)
		if lookupErr != nil {
			return 0, DatabaseError(lookupErr)
		}
		if found == nil {
			return 0, ErrCategoryNotFound
		}
		affected, err = entry.MarkAllReadByCategory(ctx, a.DB, found.ID, limits)
	default:
		return 0, ValidationError("Invalid stream for mark-all-as-read")
	}
	if err != nil {
		return 0, DatabaseError(err)
	}
	return affected, nil
}

// tagArgument is the s parameter, or t as clients' fallback.
func tagArgument(form url.Values) string {
	if s := form.Get("s"); s != "" {
		return s
	}
	return form.Get("t")
}

// labelOf parses a tag argument that must be a label.
func labelOf(raw, notLabelMessage string) (string, error) {
	stream, err := greader.ParseStream(raw)
	if err != nil {
		return "", appError(err)
	}
	if stream.Kind != greader.Label {
		return "", ValidationError(notLabelMessage)
	}
	return stream.Name, nil
}

// greaderDisableTag is POST /reader/api/0/disable-tag: deletes a label.
func (a *App) greaderDisableTag(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	raw := tagArgument(form)
	if raw == "" {
		writeError(w, ValidationError("Missing tag parameter (s or t)"))
		return
	}
	name, err := labelOf(raw, "Can only disable label tags")
	if err != nil {
		writeError(w, err)
		return
	}
	category, err := models.FindCategoryByNameAndUser(r.Context(), a.DB, name, user.user.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	if category == nil {
		writeError(w, ErrCategoryNotFound)
		return
	}
	if err := models.DeleteCategory(r.Context(), a.DB, category.ID, user.user.ID); err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	a.greaderSidebarChanged(user.user.ID)
	writeText(w, "OK")
}

// greaderRenameTag is POST /reader/api/0/rename-tag.
func (a *App) greaderRenameTag(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	if err := a.renameTag(r.Context(), user.user.ID, form); err != nil {
		writeError(w, err)
		return
	}
	a.greaderSidebarChanged(user.user.ID)
	writeText(w, "OK")
}

func (a *App) renameTag(ctx context.Context, userID int64, form url.Values) error {
	source, destination := tagArgument(form), form.Get("dest")
	if source == "" {
		return ValidationError("Missing source tag (s or t)")
	}
	if destination == "" {
		return ValidationError("Missing destination tag (dest)")
	}
	oldName, err := labelOf(source, "Can only rename label tags")
	if err != nil {
		return err
	}
	newName, err := labelOf(destination, "Destination must be a label tag")
	if err != nil {
		return err
	}
	if oldName == newName {
		_, err := a.findOrCreateCategory(ctx, userID, newName)
		return err
	}
	category, err := models.FindCategoryByNameAndUser(ctx, a.DB, oldName, userID)
	if err != nil {
		return DatabaseError(err)
	}
	if category == nil {
		return ErrCategoryNotFound
	}
	if _, err := models.RenameCategory(ctx, a.DB, category.ID, userID, newName); err != nil {
		return appError(err)
	}
	return nil
}

// findOrCreateCategory is the user's category of that name, created when
// missing.
func (a *App) findOrCreateCategory(ctx context.Context, userID int64, name string) (*models.Category, error) {
	existing, err := models.FindCategoryByNameAndUser(ctx, a.DB, name, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if existing != nil {
		return existing, nil
	}
	created, err := models.CreateCategory(ctx, a.DB, userID, name)
	if err != nil {
		return nil, appError(err)
	}
	return created, nil
}
