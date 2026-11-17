package server

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// withEntries stores three entries, oldest first, and returns their ids in
// that order. Writes run inline so the next request sees them.
func (f *signedInFixture) withEntries(t *testing.T) []int64 {
	t.Helper()
	f.app.detach = func(task func()) { task() }
	_, feedID := f.withFeed(t)
	base := time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)
	var ids []int64
	for i, guid := range []string{"a", "b", "c"} {
		title, content := "Title "+guid, "<p>body "+guid+"</p>"
		published := base.Add(time.Duration(i) * time.Hour)
		result, err := entry.UpsertEntry(context.Background(), f.database, &entry.Upsert{
			FeedID: feedID, GUID: guid, Title: &title, Content: &content, PublishedAt: &published,
		})
		if err != nil {
			t.Fatal(err)
		}
		ids = append(ids, result.ID)
	}
	return ids
}

func (f *signedInFixture) stored(t *testing.T, id int64) *entry.WithFeed {
	t.Helper()
	found, err := entry.FindByIDForUser(context.Background(), f.database, 1, id)
	if err != nil || found == nil {
		t.Fatalf("entry %d: %v %v", id, found, err)
	}
	return found
}

func decodeBody(t *testing.T, rec *httptest.ResponseRecorder) map[string]any {
	t.Helper()
	var body map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &body); err != nil {
		t.Fatalf("body %q: %v", rec.Body, err)
	}
	return body
}

func (f *signedInFixture) postAPI(path string) *httptest.ResponseRecorder {
	request := jsonRequest(http.MethodPost, path, "")
	request.Header.Set("X-CSRF-Token", f.csrf)
	request.AddCookie(f.session)
	return f.do(request)
}

func TestEntryNeighborsAndPositionFollowPublishedOrder(t *testing.T) {
	f := newSignedIn(t)
	ids := f.withEntries(t)
	middle := "/api/entries/" + itoa(ids[1])

	neighbors := decodeBody(t, f.get(middle+"/neighbors"))
	// The list is newest first, so the newer entry is "prev".
	if neighbors["prev_id"] != float64(ids[2]) || neighbors["next_id"] != float64(ids[0]) {
		t.Errorf("neighbors = %v, want prev %d next %d", neighbors, ids[2], ids[0])
	}
	newest := decodeBody(t, f.get("/api/entries/"+itoa(ids[2])+"/neighbors"))
	if newest["prev_id"] != nil || newest["next_id"] != float64(ids[1]) {
		t.Errorf("newest neighbors = %v", newest)
	}
	position := decodeBody(t, f.get(middle+"/position"))
	if position["position"] != float64(2) || position["total"] != float64(3) {
		t.Errorf("position = %v, want 2 of 3", position)
	}
}

func TestEntryNeighborsHonourFilters(t *testing.T) {
	f := newSignedIn(t)
	ids := f.withEntries(t)
	if _, err := entry.SetStarredForUser(context.Background(), f.database, 1, ids[0], true); err != nil {
		t.Fatal(err)
	}
	if _, err := entry.SetStarredForUser(context.Background(), f.database, 1, ids[2], true); err != nil {
		t.Fatal(err)
	}
	body := decodeBody(t, f.get("/api/entries/"+itoa(ids[2])+"/neighbors?starred_only=true"))
	if body["next_id"] != float64(ids[0]) {
		t.Errorf("starred neighbors = %v, want next %d", body, ids[0])
	}
	position := decodeBody(t, f.get("/api/entries/"+itoa(ids[0])+"/position?starred_only=true"))
	if position["position"] != float64(2) || position["total"] != float64(2) {
		t.Errorf("starred position = %v", position)
	}
	if rec := f.get("/api/entries/" + itoa(ids[0]) + "/neighbors?unread_only=maybe"); rec.Code != http.StatusBadRequest {
		t.Errorf("bad flag: %d, want 400", rec.Code)
	}
}

func TestStarAndReadFormsChangeTheEntryAndAnswerAFragment(t *testing.T) {
	f := newSignedIn(t)
	id := f.withEntries(t)[0]
	base := "/entries/" + itoa(id)
	cases := []struct {
		action string
		check  func(*entry.WithFeed) bool
	}{
		{"/star", func(e *entry.WithFeed) bool { return e.StarredAt != nil }},
		{"/unstar", func(e *entry.WithFeed) bool { return e.StarredAt == nil }},
		{"/read", func(e *entry.WithFeed) bool { return e.ReadAt != nil }},
		{"/unread", func(e *entry.WithFeed) bool { return e.ReadAt == nil }},
	}
	for _, c := range cases {
		rec := f.postForm(base+c.action, url.Values{})
		if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "<") {
			t.Errorf("%s: %d %q, want an HTML fragment", c.action, rec.Code, rec.Body)
		}
		if !c.check(f.stored(t, id)) {
			t.Errorf("%s did not change the entry", c.action)
		}
	}
}

func TestMarkingUnreadToastsOnlyOnARealChange(t *testing.T) {
	f := newSignedIn(t)
	id := f.withEntries(t)[0]
	base := "/entries/" + itoa(id)
	if body := f.postForm(base+"/unread", url.Values{}).Body.String(); strings.Contains(body, "Marked as unread.") {
		t.Error("unread on an unread entry toasted")
	}
	f.postForm(base+"/read", url.Values{})
	if body := f.postForm(base+"/unread", url.Values{}).Body.String(); !strings.Contains(body, "Marked as unread.") {
		t.Errorf("unread after read lacks the toast: %q", body)
	}
}

func TestEntryActionFromAPlainFormRedirectsBackToTheList(t *testing.T) {
	f := newSignedIn(t)
	id := f.withEntries(t)[0]
	request := httptest.NewRequest(http.MethodPost, "/entries/"+itoa(id)+"/star", nil)
	request.Header.Set("Sec-Fetch-Dest", "document")
	request.Header.Set("X-CSRF-Token", f.csrf)
	request.AddCookie(f.session)
	rec := f.do(request)
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/entries?entry="+itoa(id) {
		t.Errorf("%d %q, want 303 to the entry", rec.Code, rec.Header().Get("Location"))
	}
	if f.stored(t, id).StarredAt == nil {
		t.Error("star was not stored")
	}
}

func TestEntryActionOnAnotherUsersEntryIs404(t *testing.T) {
	f := newSignedIn(t)
	f.withEntries(t)
	for _, action := range []string{"star", "read", "summarize", "summarize/cancel", "save", "fetch-full-content"} {
		if rec := f.postForm("/entries/999/"+action, url.Values{}); rec.Code != http.StatusNotFound {
			t.Errorf("%s: %d, want 404", action, rec.Code)
		}
	}
}

func TestSummarizeFormQueuesAPendingSummaryAndCancelClearsIt(t *testing.T) {
	f := newSignedIn(t)
	id := f.withEntries(t)[0]
	base := "/entries/" + itoa(id)
	if rec := f.postForm(base+"/summarize", url.Values{}); rec.Code != http.StatusOK {
		t.Fatalf("summarize: %d %s", rec.Code, rec.Body)
	}
	stored, err := models.FindEntrySummary(context.Background(), f.database, 1, id)
	if err != nil || stored == nil || stored.Status != models.SummaryPending {
		t.Fatalf("stored summary = %+v, %v; want pending", stored, err)
	}
	select {
	case job := <-f.app.SummaryQueue:
		if job.UserID != 1 || job.EntryID != id {
			t.Errorf("job = %+v", job)
		}
	default:
		t.Error("no job was queued")
	}
	if rec := f.postForm(base+"/summarize/cancel", url.Values{}); rec.Code != http.StatusOK {
		t.Fatalf("cancel: %d %s", rec.Code, rec.Body)
	}
	if stored, _ := models.FindEntrySummary(context.Background(), f.database, 1, id); stored != nil {
		t.Errorf("summary survived the cancel: %+v", stored)
	}
}

func TestSummaryAPILifecycle(t *testing.T) {
	f := newSignedIn(t)
	id := f.withEntries(t)[0]
	path := "/api/entries/" + itoa(id) + "/summary"
	if rec := f.get(path); rec.Code != http.StatusNotFound {
		t.Fatalf("no summary yet: %d, want 404", rec.Code)
	}
	ctx := context.Background()
	if _, err := models.UpsertPendingSummary(ctx, f.database, 1, id); err != nil {
		t.Fatal(err)
	}
	if _, err := models.SetSummaryCompleted(ctx, f.database, 1, id, "Short text"); err != nil {
		t.Fatal(err)
	}
	body := decodeBody(t, f.get(path))
	if body["status"] != "completed" || body["summary_text"] != "Short text" {
		t.Errorf("summary = %v", body)
	}
	// An existing summary is returned as is, never queued again.
	if again := decodeBody(t, f.postAPI("/api/entries/"+itoa(id)+"/summarize")); again["status"] != "completed" {
		t.Errorf("summarize of a finished summary = %v", again)
	}
	del := jsonRequest(http.MethodDelete, path, "")
	del.Header.Set("X-CSRF-Token", f.csrf)
	del.AddCookie(f.session)
	if rec := f.do(del); rec.Code != http.StatusOK || decodeBody(t, rec)["success"] != true {
		t.Errorf("delete: %d %s", rec.Code, rec.Body)
	}
	if rec := f.get(path); rec.Code != http.StatusNotFound {
		t.Errorf("after delete: %d, want 404", rec.Code)
	}
}

func TestSummarizeAPIWithoutKagiIsRefusedAndQueuesNothing(t *testing.T) {
	f := newSignedIn(t)
	id := f.withEntries(t)[0]
	// The fixture entries carry no link, so the link check answers first.
	rec := f.postAPI("/api/entries/" + itoa(id) + "/summarize")
	if rec.Code != http.StatusUnprocessableEntity && rec.Code != http.StatusBadRequest {
		t.Errorf("status %d %s, want a validation error", rec.Code, rec.Body)
	}
	if len(f.app.SummaryQueue) != 0 {
		t.Error("a job was queued")
	}
}

func TestSaveAndFetchFullContentRefuseAnEntryWithoutALink(t *testing.T) {
	f := newSignedIn(t)
	id := f.withEntries(t)[0]
	for _, action := range []string{"save", "fetch-full-content"} {
		if rec := f.postAPI("/api/entries/" + itoa(id) + "/" + action); rec.Code != http.StatusUnprocessableEntity && rec.Code != http.StatusBadRequest {
			t.Errorf("api %s: %d %s", action, rec.Code, rec.Body)
		}
	}
}
