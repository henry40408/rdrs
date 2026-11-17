package feedparse

import "strings"

func (p *parser) rss(root *element, kind Type) (*Feed, error) {
	for _, child := range root.children() {
		if _, name := p.tag(child); name == "channel" {
			return p.rssChannel(child, kind), nil
		}
	}
	return nil, ErrNoFeed
}

func (p *parser) rssChannel(channel *element, kind Type) *Feed {
	feed := &Feed{Type: kind}
	for _, child := range channel.children() {
		ns, name := p.tag(child)
		switch {
		case ns == nsRSS && name == "title":
			feed.Title = titleOf(child)
		case ns == nsRSS && name == "link":
			if link, ok := linkOf(child, TargetNone); ok {
				feed.Links = append(feed.Links, link)
			}
		case ns == nsAtom && name == "link":
			if link, ok := atomLink(child); ok {
				feed.Links = append(feed.Links, link)
			}
		case ns == nsRSS && name == "description":
			feed.Description = titleOf(child)
		case ns == nsRSS && name == "pubDate":
			feed.Published = timestampOf(child)
		case ns == nsRSS && (name == "lastBuildDate" || name == "updated"):
			feed.Updated = timestampOf(child)
		case ns == nsRSS && name == "image":
			feed.Logo = p.rssImage(child)
		case ns == nsRSS && name == "item":
			feed.Entries = append(feed.Entries, p.rssItem(child))
		case ns == nsItunes && name == "image":
			if href, ok := child.attr("href"); ok && feed.Logo == nil {
				feed.Logo = &href
			}
		}
	}
	return feed
}

// rssImage is the channel <image>; with no url there is no image.
func (p *parser) rssImage(image *element) *string {
	var uri string
	for _, child := range image.children() {
		if ns, name := p.tag(child); ns == nsRSS && name == "url" {
			if text, ok := child.text(); ok {
				uri = text
			}
		}
	}
	if uri == "" {
		return nil
	}
	return &uri
}

func (p *parser) rssItem(item *element) Entry {
	var entry Entry
	for _, child := range item.children() {
		ns, name := p.tag(child)
		switch {
		case ns == nsRSS && name == "title":
			entry.Title = titleOf(child)
		case ns == nsRSS && name == "link":
			if link, ok := linkOf(child, TargetNone); ok {
				entry.Links = append(entry.Links, link)
			}
		case ns == nsRSS && name == "description":
			entry.Summary = titleOf(child)
		case ns == nsRSS && name == "author":
			if text, ok := child.text(); ok {
				entry.Authors = append(entry.Authors, parsePerson(text))
			}
		case ns == nsRSS && name == "guid":
			if text, ok := child.text(); ok {
				entry.ID = strings.TrimSpace(text)
			}
		case (ns == nsRSS && name == "pubDate") || (ns == nsDublinCore && name == "date"):
			entry.Published = timestampOf(child)
		case (ns == nsRSS || ns == nsAtom) && name == "updated":
			entry.Updated = timestampOf(child)
		case ns == nsContent && name == "encoded":
			entry.Content = contentEncoded(child)
		case ns == nsDublinCore && name == "creator":
			if text, ok := child.text(); ok {
				entry.Authors = append(entry.Authors, parsePerson(text))
			}
		case ns == nsCommentAPI && (name == "commentRss" || name == "commentRSS"):
			if link, ok := linkOf(child, TargetCommentsFeed); ok {
				entry.Links = append(entry.Links, link)
			}
		case ns == nsRSS && name == "comments":
			if link, ok := linkOf(child, TargetComments); ok {
				entry.Links = append(entry.Links, link)
			}
		}
	}
	if entry.Updated == nil {
		entry.Updated = entry.Published
	}
	entry.Links = dropRedundantCommentLinks(entry.Links)
	return entry
}

// contentEncoded is <content:encoded>, which keeps its text untrimmed; empty
// is no content.
func contentEncoded(e *element) *string {
	text, ok := e.text()
	if !ok || text == "" {
		return nil
	}
	return &text
}

// dropRedundantCommentLinks removes a comments link that points where
// another link already does, so it cannot become the entry's link.
func dropRedundantCommentLinks(links []Link) []Link {
	seen := map[string]bool{}
	for _, l := range links {
		if l.Target != TargetComments {
			seen[l.Href] = true
		}
	}
	var kept []Link
	for _, l := range links {
		if l.Target == TargetComments && seen[l.Href] {
			continue
		}
		kept = append(kept, l)
	}
	return kept
}
