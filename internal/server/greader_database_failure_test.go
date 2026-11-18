package server

import (
	"net/http"
	"net/url"
	"slices"
	"testing"
)

type greaderCall struct {
	method, path string
	form         url.Values
	// goneBreaks lists the tables whose absence must make the call answer
	// 500; refusedBreaks lists those whose refused writes must.
	goneBreaks, refusedBreaks []string
}

const (
	grReadingList = "user/-/state/com.google/reading-list"
	grFeedStream  = "feed/https://example.com/feed.xml"
	grLabelStream = "user/-/label/Tech"
)

func greaderCalls(item string) []greaderCall {
	all := []string{"entry", "feed", "category"}
	return []greaderCall{
		{method: "GET", path: "subscription/list?output=json", goneBreaks: []string{"feed", "category"}},
		{method: "GET", path: "subscription/export", goneBreaks: []string{"feed", "category"}},
		{method: "GET", path: "subscribed?s=" + grFeedStream, goneBreaks: []string{"feed", "category"}},
		{method: "GET", path: "stream/contents/" + grReadingList, goneBreaks: append([]string{"user_settings"}, all...)},
		{method: "GET", path: "stream/contents/" + grFeedStream, goneBreaks: append([]string{"user_settings"}, all...)},
		{method: "GET", path: "stream/contents/" + grLabelStream, goneBreaks: append([]string{"user_settings"}, all...)},
		{method: "GET", path: "stream/contents/user/-/state/com.google/starred", goneBreaks: append([]string{"user_settings"}, all...)},
		{method: "GET", path: "stream/items/ids?n=10&s=" + grReadingList, goneBreaks: all},
		{method: "GET", path: "stream/items/ids?n=10&s=user/-/state/com.google/starred", goneBreaks: all},
		{method: "GET", path: "stream/items/ids?n=10&s=" + grLabelStream, goneBreaks: all},
		{method: "GET", path: "stream/items/count?s=" + grReadingList, goneBreaks: all},
		{method: "GET", path: "stream/items/contents?i=" + item, goneBreaks: append([]string{"user_settings"}, all...)},
		{method: "POST", path: "stream/items/contents", form: url.Values{"i": {item}}, goneBreaks: append([]string{"user_settings"}, all...)},
		{method: "GET", path: "tag/list", goneBreaks: []string{"category"}},
		{method: "GET", path: "unread-count", goneBreaks: all},
		{method: "POST", path: "edit-tag", form: url.Values{"i": {item}, "a": {"user/-/state/com.google/read"}}, goneBreaks: all, refusedBreaks: []string{"entry"}},
		{method: "POST", path: "edit-tag", form: url.Values{"i": {item}, "r": {"user/-/state/com.google/read"}}, goneBreaks: all, refusedBreaks: []string{"entry"}},
		{method: "POST", path: "edit-tag", form: url.Values{"i": {item}, "a": {"user/-/state/com.google/starred"}}, goneBreaks: all, refusedBreaks: []string{"entry"}},
		{method: "POST", path: "mark-all-as-read", form: url.Values{"s": {grReadingList}}, goneBreaks: all, refusedBreaks: []string{"entry"}},
		{method: "POST", path: "mark-all-as-read", form: url.Values{"s": {grFeedStream}}, goneBreaks: all, refusedBreaks: []string{"entry"}},
		{method: "POST", path: "mark-all-as-read", form: url.Values{"s": {grLabelStream}}, goneBreaks: all, refusedBreaks: []string{"entry"}},
		{method: "POST", path: "disable-tag", form: url.Values{"s": {grLabelStream}}, goneBreaks: []string{"category"}, refusedBreaks: all},
		{method: "POST", path: "rename-tag", form: url.Values{"s": {grLabelStream}, "dest": {"user/-/label/Other"}}, goneBreaks: []string{"category"}, refusedBreaks: []string{"category"}},
		{method: "POST", path: "subscription/edit", form: url.Values{"ac": {"unsubscribe"}, "s": {grFeedStream}}, goneBreaks: []string{"feed", "category"}, refusedBreaks: []string{"entry", "feed"}},
		{method: "POST", path: "subscription/edit", form: url.Values{"ac": {"edit"}, "s": {grFeedStream}, "t": {"New"}, "a": {grLabelStream}}, goneBreaks: []string{"feed", "category"}, refusedBreaks: []string{"feed"}},
	}
}

// TestGReaderAnswers500WhenTheDatabaseFails runs every storage-backed GReader
// call against a missing table and against a table that refuses writes. A
// client must see a server error, never an empty success it would sync as
// "nothing changed".
func TestGReaderAnswers500WhenTheDatabaseFails(t *testing.T) {
	for _, table := range []string{"entry", "feed", "category", "user_settings"} {
		for _, mode := range []string{"gone", "refused"} {
			t.Run(mode+"/"+table, func(t *testing.T) {
				f := newGReader(t)
				_, feedID := f.withFeed(t)
				item := itemRef(f.entries(t, feedID, 2)[0])
				if mode == "gone" {
					execOn(t, f, "ALTER TABLE "+table+" RENAME TO "+table+"_gone")
				} else {
					for _, event := range []string{"INSERT", "UPDATE", "DELETE"} {
						execOn(t, f, "CREATE TRIGGER refuse_"+event+" BEFORE "+event+" ON "+table+" BEGIN SELECT RAISE(ABORT, 'refused'); END")
					}
				}
				for _, call := range greaderCalls(item) {
					breaks := call.goneBreaks
					if mode == "refused" {
						breaks = call.refusedBreaks
					}
					if !slices.Contains(breaks, table) {
						continue
					}
					path := grBase + call.path
					recorder := f.browserGet(path)
					if call.method == http.MethodPost {
						recorder = f.browserPost(path, call.form)
					}
					if recorder.Code != http.StatusInternalServerError {
						t.Errorf("%s %s = %d %.80s, want 500", call.method, call.path, recorder.Code, recorder.Body)
					}
				}
			})
		}
	}
}

func execOn(t *testing.T, f *grFixture, query string) {
	t.Helper()
	if _, err := f.database.ExecContext(t.Context(), query); err != nil {
		t.Fatal(err)
	}
}
