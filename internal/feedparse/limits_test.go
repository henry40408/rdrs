package feedparse

import (
	"strings"
	"testing"
	"time"
)

func TestDeepNestingParsesInLinearTime(t *testing.T) {
	const depth = 100_000
	doc := `<rss version="2.0"><channel><title>T</title><item><title>I</title><description>` +
		strings.Repeat("<b>", depth) + strings.Repeat("</b>", depth) +
		`</description></item></channel></rss>`
	start := time.Now()
	feed := mustParse(t, doc)
	if len(feed.Entries) != 1 {
		t.Fatalf("entries = %d", len(feed.Entries))
	}
	if elapsed := time.Since(start); elapsed > 3*time.Second {
		t.Errorf("deep document took %v; namespace lookup is not constant time", elapsed)
	}
}

func TestNamespaceScopesRestoreAfterTheElementCloses(t *testing.T) {
	feed := mustParse(t, `<feed xmlns="http://www.w3.org/2005/Atom"><title>T</title>`+
		`<entry xmlns="urn:other"><id>x</id></entry><entry><id>2</id><title>Two</title></entry></feed>`)
	if len(feed.Entries) != 1 || feed.Entries[0].Title == nil || *feed.Entries[0].Title != "Two" {
		t.Fatalf("entries = %+v", feed.Entries)
	}
}

func TestDecodeIgnoresHTMLMetaCharset(t *testing.T) {
	body := `<rss><channel><title><![CDATA[<meta charset="iso-8859-1">é]]></title></channel></rss>`
	got, err := Decode([]byte(body), "")
	if err != nil || string(got) != body {
		t.Fatalf("Decode = %q, %v", got, err)
	}
}

func TestTooManyElementsIsAParseError(t *testing.T) {
	doc := `<rss version="2.0"><channel><title>T</title>` + strings.Repeat("<x/>", maxElements) + `</channel></rss>`
	if _, err := Parse([]byte(doc)); err == nil || !strings.Contains(err.Error(), "too many elements") {
		t.Fatalf("err = %v", err)
	}
}
