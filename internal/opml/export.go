// Package opml exports subscriptions as OPML and imports them back.
package opml

import (
	"strings"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/textutil"
)

// attributeEscaper escapes the five characters quick-xml escapes in an
// attribute value, so the output is byte-identical to the Rust export.
var attributeEscaper = strings.NewReplacer(
	"&", "&amp;", "<", "&lt;", ">", "&gt;", `"`, "&quot;", "'", "&apos;",
)

// Export writes the feeds grouped by category. A category without feeds is
// left out. Names are entity-decoded first because older rows hold the
// double-encoded text some feeds produce.
func Export(categories []models.Category, feeds []models.Feed) string {
	var out strings.Builder
	out.WriteString("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
	out.WriteString("<opml version=\"2.0\">\n<head>\n<title>RDRS Subscriptions</title>\n</head>\n<body>\n")
	for _, category := range categories {
		writeCategory(&out, category, feedsOf(category, feeds))
	}
	out.WriteString("</body>\n</opml>")
	return out.String()
}

func feedsOf(category models.Category, feeds []models.Feed) []models.Feed {
	var own []models.Feed
	for _, feed := range feeds {
		if feed.CategoryID == category.ID {
			own = append(own, feed)
		}
	}
	return own
}

func writeCategory(out *strings.Builder, category models.Category, feeds []models.Feed) {
	if len(feeds) == 0 {
		return
	}
	name := attribute(textutil.DecodeHTMLEntities(category.Name))
	out.WriteString(`<outline text="` + name + `" title="` + name + "\">\n")
	for _, feed := range feeds {
		writeFeed(out, feed)
	}
	out.WriteString("</outline>\n")
}

func writeFeed(out *strings.Builder, feed models.Feed) {
	title := feed.URL
	if feed.Title != nil {
		title = *feed.Title
	}
	decodedTitle := attribute(textutil.DecodeHTMLEntities(title))
	out.WriteString(`<outline type="rss" text="` + decodedTitle + `" title="` + decodedTitle +
		`" xmlUrl="` + attribute(textutil.DecodeHTMLEntities(feed.URL)) + `"`)
	if feed.SiteURL != nil {
		out.WriteString(` htmlUrl="` + attribute(textutil.DecodeHTMLEntities(*feed.SiteURL)) + `"`)
	}
	out.WriteString("/>\n")
}

func attribute(value string) string {
	return attributeEscaper.Replace(value)
}
