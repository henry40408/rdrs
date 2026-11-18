package harness

import "testing"

// A faded-out match ahead of a visible twin must not hide the twin: the
// categories page has an invisible "save" button before its "rename" label,
// both .action-link.
func TestCSSReturnsTheFirstDisplayedMatchNotTheFirstMatch(t *testing.T) {
	browser, err := Open()
	if err != nil {
		t.Fatal(err)
	}
	defer browser.Close()
	page := `data:text/html,<button class="twin" style="display:none">hidden</button><button class="twin" id="shown">shown</button>`
	if err := browser.Goto(page); err != nil {
		t.Fatal(err)
	}

	element, err := browser.CSS(".twin")
	if err != nil {
		t.Fatal(err)
	}
	id, _, err := element.Attr("id")
	if err != nil {
		t.Fatal(err)
	}
	if id != "shown" {
		t.Fatalf("CSS returned the element with id %q, want the displayed one", id)
	}
}
