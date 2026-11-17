package feedparse

func (p *parser) rdf(root *element) (*Feed, error) {
	feed := &Feed{Type: TypeRSS1}
	for _, child := range root.children() {
		ns, name := p.tag(child)
		switch {
		case ns == nsRSS && name == "channel":
			p.rdfChannel(feed, child)
		case ns == nsRSS && name == "image":
			feed.Logo = p.rssImage(child)
		case ns == nsRSS && name == "item":
			if entry, ok := p.rdfItem(child); ok {
				feed.Entries = append(feed.Entries, entry)
			}
		}
	}
	return feed, nil
}

func (p *parser) rdfChannel(feed *Feed, channel *element) {
	for _, child := range channel.children() {
		ns, name := p.tag(child)
		switch {
		case ns == nsRSS && name == "title":
			feed.Title = titleOf(child)
		case ns == nsRSS && name == "link":
			if link, ok := linkOf(child, TargetNone); ok {
				feed.Links = append(feed.Links, link)
			}
		case ns == nsRSS && name == "description":
			feed.Description = titleOf(child)
		case ns == nsDublinCore && name == "date":
			feed.Published = timestampOf(child)
		}
	}
}

// rdfItem drops an item with no link: there is nowhere to send the reader.
func (p *parser) rdfItem(item *element) (Entry, bool) {
	var entry Entry
	var encoded *string
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
		case ns == nsContent && name == "encoded":
			if text, ok := child.text(); ok {
				encoded = ptr(newText(text))
			}
		case ns == nsDublinCore && name == "creator":
			if text, ok := child.text(); ok {
				entry.Authors = append(entry.Authors, parsePerson(text))
			}
		case ns == nsDublinCore && name == "date":
			entry.Published = timestampOf(child)
		case ns == nsDublinCore && name == "description" && entry.Summary == nil:
			entry.Summary = titleOf(child)
		}
	}
	entry.Content = encoded
	return entry, len(entry.Links) > 0
}
