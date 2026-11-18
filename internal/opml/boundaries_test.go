package opml

import "testing"

func TestCategoryEndsAfterItsClosingTag(t *testing.T) {
	outlines, err := Parse(`<opml><body><outline xmlUrl="http://a"></outline>` +
		`<outline text="C"><outline xmlUrl="http://b"/></outline><outline xmlUrl="http://u"/></body></opml>`)
	if err != nil {
		t.Fatal(err)
	}
	want := []struct{ category, feed string }{{"Uncategorized", "http://a"}, {"C", "http://b"}, {"Uncategorized", "http://u"}}
	if len(outlines) != len(want) {
		t.Fatalf("outlines = %+v", outlines)
	}
	for i, w := range want {
		if outlines[i].CategoryName != w.category || len(outlines[i].Feeds) != 1 || outlines[i].Feeds[0].XMLURL != w.feed {
			t.Errorf("outline %d = %+v, want %s/%s", i, outlines[i], w.category, w.feed)
		}
	}
}
