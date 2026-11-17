package greader

// The JSON shapes of the API, field for field and in the order of the Rust
// serde structs. A "_"-prefixed field is an rdrs extension that Google Reader
// clients ignore. A pointer with omitempty is a Rust Option that is skipped
// when None; a pointer without it is written as null.

// Item is one entry of stream/contents.
type Item struct {
	ID            string          `json:"id"`
	Published     int64           `json:"published"`
	Updated       int64           `json:"updated"`
	CrawlTimeMsec string          `json:"crawlTimeMsec"`
	TimestampUsec string          `json:"timestampUsec"`
	Title         string          `json:"title"`
	Categories    []string        `json:"categories"`
	Summary       Content         `json:"summary"`
	Canonical     []Link          `json:"canonical"`
	Alternate     []AlternateLink `json:"alternate"`
	Author        string          `json:"author"`
	Origin        Origin          `json:"origin"`

	EntryID       int64   `json:"_entryId"`
	FeedID        int64   `json:"_feedId"`
	CategoryID    int64   `json:"_categoryId"`
	CategoryName  string  `json:"_categoryName"`
	FeedHasIcon   bool    `json:"_feedHasIcon"`
	ReadAt        *string `json:"_readAt"`
	StarredAt     *string `json:"_starredAt"`
	PublishedAt   *string `json:"_publishedAt"`
	FullContent   *string `json:"_content"`
	SummaryStatus *string `json:"_summaryStatus,omitempty"`
}

// Content is the HTML body of an item.
type Content struct {
	Content string `json:"content"`
}

// Link is a canonical link.
type Link struct {
	Href string `json:"href"`
}

// AlternateLink is an alternate link with its media type.
type AlternateLink struct {
	Href string `json:"href"`
	Type string `json:"type"`
}

// Origin names the feed an item came from.
type Origin struct {
	StreamID string `json:"streamId"`
	Title    string `json:"title"`
	HTMLURL  string `json:"htmlUrl"`
}

// StreamContents is the response of stream/contents.
type StreamContents struct {
	ID           string  `json:"id"`
	Updated      int64   `json:"updated"`
	Continuation *string `json:"continuation,omitempty"`
	Items        []Item  `json:"items"`
}

// StreamItemIDs is the response of stream/items/ids.
type StreamItemIDs struct {
	ItemRefs     []ItemRef `json:"itemRefs"`
	Continuation *string   `json:"continuation,omitempty"`
}

// ItemRef is an item id with its timestamp.
type ItemRef struct {
	ID            string `json:"id"`
	TimestampUsec string `json:"timestampUsec"`
}

// UnreadCounts is the response of unread-count.
type UnreadCounts struct {
	Max          int64         `json:"max"`
	UnreadCounts []UnreadCount `json:"unreadcounts"`
}

// UnreadCount is the unread total of one stream.
type UnreadCount struct {
	ID                      string `json:"id"`
	Count                   int64  `json:"count"`
	NewestItemTimestampUsec string `json:"newestItemTimestampUsec"`
}

// SubscriptionList is the response of subscription/list.
type SubscriptionList struct {
	Subscriptions []Subscription `json:"subscriptions"`
}

// Subscription is one feed of subscription/list.
type Subscription struct {
	ID         string                 `json:"id"`
	Title      string                 `json:"title"`
	Categories []SubscriptionCategory `json:"categories"`
	SortID     string                 `json:"sortid"`
	HTMLURL    string                 `json:"htmlUrl"`
	URL        string                 `json:"url"`
	IconURL    string                 `json:"iconUrl"`

	FeedID          int64   `json:"_feedId"`
	FetchError      *string `json:"_fetchError,omitempty"`
	Description     *string `json:"_description,omitempty"`
	CustomUserAgent *string `json:"_customUserAgent,omitempty"`
	HTTP2Disabled   bool    `json:"_http2Disabled"`
	CustomReferrer  *string `json:"_customReferrer,omitempty"`
}

// SubscriptionCategory is the label a subscription is filed under.
type SubscriptionCategory struct {
	ID    string `json:"id"`
	Label string `json:"label"`
}

// TagList is the response of tag/list.
type TagList struct {
	Tags []Tag `json:"tags"`
}

// Tag is a state or folder tag.
type Tag struct {
	ID     string  `json:"id"`
	Type   *string `json:"type,omitempty"`
	SortID *string `json:"sortid,omitempty"`
}

// UserInfo is the response of user-info.
type UserInfo struct {
	UserID        string `json:"userId"`
	UserName      string `json:"userName"`
	UserProfileID string `json:"userProfileId"`
	UserEmail     string `json:"userEmail"`
}

// QuickAdd is the response of subscription/quickadd.
type QuickAdd struct {
	NumResults int    `json:"numResults"`
	Query      string `json:"query"`
	StreamID   string `json:"streamId"`
}
