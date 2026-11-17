package feedparse

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

type jsonAuthor struct {
	Name *string `json:"name"`
}

type jsonAttachment struct {
	URL      *string `json:"url"`
	MimeType *string `json:"mime_type"`
}

type jsonItem struct {
	ID            *string          `json:"id"`
	URL           *string          `json:"url"`
	ExternalURL   *string          `json:"external_url"`
	Title         *string          `json:"title"`
	ContentHTML   *string          `json:"content_html"`
	ContentText   *string          `json:"content_text"`
	Summary       *string          `json:"summary"`
	DatePublished *string          `json:"date_published"`
	DateModified  *string          `json:"date_modified"`
	Author        *jsonAuthor      `json:"author"`
	Authors       []jsonAuthor     `json:"authors"`
	Attachments   []jsonAttachment `json:"attachments"`
}

type jsonFeed struct {
	Version     *string      `json:"version"`
	Title       *string      `json:"title"`
	HomePageURL *string      `json:"home_page_url"`
	FeedURL     *string      `json:"feed_url"`
	Description *string      `json:"description"`
	Icon        *string      `json:"icon"`
	Favicon     *string      `json:"favicon"`
	Author      *jsonAuthor  `json:"author"`
	Authors     []jsonAuthor `json:"authors"`
	Items       *[]jsonItem  `json:"items"`
}

func jsonError(detail string) error {
	return errors.New("unable to parse JSON: " + detail)
}

func parseJSON(data []byte) (*Feed, error) {
	var doc jsonFeed
	if err := json.Unmarshal(data, &doc); err != nil {
		return nil, jsonError(err.Error())
	}
	for field, missing := range map[string]bool{
		"version": doc.Version == nil, "title": doc.Title == nil, "items": doc.Items == nil,
	} {
		if missing {
			return nil, jsonError("missing field `" + field + "`")
		}
	}
	if !strings.HasPrefix(*doc.Version, "https://jsonfeed.org/version/1") {
		return nil, fmt.Errorf("unsupported version: %s", *doc.Version)
	}
	for _, item := range *doc.Items {
		for _, a := range item.Attachments {
			if a.URL == nil || a.MimeType == nil {
				return nil, jsonError("missing field in attachment")
			}
		}
	}
	return convertJSON(&doc), nil
}

func convertJSON(doc *jsonFeed) *Feed {
	feed := &Feed{Type: TypeJSON, Title: ptr(newText(*doc.Title)), Logo: doc.Icon, Icon: doc.Favicon}
	for _, uri := range []*string{doc.HomePageURL, doc.FeedURL} {
		if uri != nil {
			feed.Links = append(feed.Links, newLink(*uri, nil))
		}
	}
	if doc.Description != nil {
		feed.Description = ptr(newText(*doc.Description))
	}
	feedAuthors := jsonAuthors(nil, doc.Author, doc.Authors)
	for _, item := range *doc.Items {
		entry := convertJSONItem(item)
		if len(entry.Authors) == 0 {
			entry.Authors = feedAuthors
		}
		feed.Entries = append(feed.Entries, entry)
	}
	return feed
}

func convertJSONItem(item jsonItem) Entry {
	var entry Entry
	if item.ID != nil {
		entry.ID = *item.ID
	}
	for _, uri := range []*string{item.URL, item.ExternalURL} {
		if uri != nil {
			entry.Links = append(entry.Links, newLink(*uri, nil))
		}
	}
	if item.Title != nil {
		entry.Title = ptr(newText(*item.Title))
	}
	setJSONContent(&entry, item)
	if item.DatePublished != nil {
		entry.Published = timestamp(*item.DatePublished)
	}
	if item.DateModified != nil {
		entry.Updated = timestamp(*item.DateModified)
	}
	entry.Authors = jsonAuthors(nil, item.Author, item.Authors)
	for _, a := range item.Attachments {
		entry.Links = append(entry.Links, newLink(*a.URL, nil))
	}
	return entry
}

// setJSONContent prefers HTML, then an explicit summary; plain text fills
// whichever is still empty.
func setJSONContent(entry *Entry, item jsonItem) {
	if item.ContentHTML != nil {
		entry.Content = ptr(strings.TrimSpace(*item.ContentHTML))
	}
	if item.Summary != nil {
		entry.Summary = ptr(newText(*item.Summary))
	}
	if item.ContentText == nil {
		return
	}
	text := strings.TrimSpace(*item.ContentText)
	switch {
	case entry.Content == nil:
		entry.Content = &text
	case entry.Summary == nil:
		entry.Summary = ptr(newText(text))
	}
}

// jsonAuthors adds each named author once.
func jsonAuthors(authors []Person, one *jsonAuthor, many []jsonAuthor) []Person {
	for _, a := range append([]jsonAuthor{*orEmpty(one)}, many...) {
		if a.Name == nil || hasName(authors, *a.Name) {
			continue
		}
		authors = append(authors, parsePerson(*a.Name))
	}
	return authors
}

func orEmpty(a *jsonAuthor) *jsonAuthor {
	if a == nil {
		return &jsonAuthor{}
	}
	return a
}

func hasName(authors []Person, name string) bool {
	for _, a := range authors {
		if a.Name != nil && *a.Name == name {
			return true
		}
	}
	return false
}
