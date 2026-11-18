package save

import "testing"

func TestLinkdingAcceptsStatus200And299(t *testing.T) {
	for _, status := range []int{200, 299} {
		result, err := saveTo(t, linkdingServer(t, status, `{"id":1}`, nil), BookmarkData{URL: "https://example.com/p"})
		if err != nil || !result.Success {
			t.Errorf("status %d: result = %+v, err = %v", status, result, err)
		}
	}
}

func TestBookmarkPageURLWhenTheSettingStartsWithAPI(t *testing.T) {
	if got := bookmarkPageURL("/api", 5); got != "/bookmarks/5" {
		t.Errorf("bookmarkPageURL = %q", got)
	}
}
