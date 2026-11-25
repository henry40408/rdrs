package feedparse

// feedVector pairs a feed document with what Parse must report for it.
type feedVector struct {
	name string
	xml  string
	want vectorFeed
}

var feedVectors = []feedVector{
	{
		name: "01-rss-basic",
		xml: `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/" xmlns:dc="http://purl.org/dc/elements/1.1/">
<channel>
<title>Basic Blog</title>
<link>https://basic.example/</link>
<description>A basic blog</description>
<lastBuildDate>Tue, 05 Mar 2024 10:20:30 GMT</lastBuildDate>
<image><url>https://basic.example/logo.png</url><title>Basic Blog</title><link>https://basic.example/</link></image>
<item>
<title>First post</title>
<link>https://basic.example/1</link>
<guid isPermaLink="true">https://basic.example/1</guid>
<description>Summary one</description>
<content:encoded><![CDATA[<p>Full <b>content</b> one</p>]]></content:encoded>
<dc:creator>Alice</dc:creator>
<pubDate>Mon, 04 Mar 2024 08:00:00 +0000</pubDate>
</item>
<item>
<title>Second post</title>
<link>https://basic.example/2</link>
<guid>post-2</guid>
<description>Only a description</description>
<author>bob@example.com (Bob Builder)</author>
<pubDate>Sun, 03 Mar 2024 08:00:00 -0500</pubDate>
</item>
<item>
<title>Third post</title>
<link>https://basic.example/3</link>
<guid>post-3</guid>
<author>carol@example.com</author>
</item>
</channel>
</rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "3353b60f96b4e62955c18f7f53f71eb1",
			Title:       ptr("Basic Blog"),
			Description: ptr("A basic blog"),
			Updated:     ptr("2024-03-05T10:20:30Z"),
			Links:       []string{"https://basic.example/"},
			Logo:        ptr("https://basic.example/logo.png"),
			Entries: []vectorEntry{
				{
					GUID:      "https://basic.example/1",
					Title:     ptr("First post"),
					Link:      ptr("https://basic.example/1"),
					Links:     []string{"https://basic.example/1"},
					Content:   ptr("<p>Full <b>content</b> one</p>"),
					Summary:   ptr("Summary one"),
					Author:    ptr("Alice"),
					Published: ptr("2024-03-04T08:00:00Z"),
					Updated:   ptr("2024-03-04T08:00:00Z"),
				},
				{
					GUID:      "post-2",
					Title:     ptr("Second post"),
					Link:      ptr("https://basic.example/2"),
					Links:     []string{"https://basic.example/2"},
					Content:   ptr("Only a description"),
					Summary:   ptr("Only a description"),
					Author:    ptr("(Bob Builder)"),
					Published: ptr("2024-03-03T13:00:00Z"),
					Updated:   ptr("2024-03-03T13:00:00Z"),
				},
				{
					GUID:  "post-3",
					Title: ptr("Third post"),
					Link:  ptr("https://basic.example/3"),
					Links: []string{"https://basic.example/3"},
				},
			},
		},
	},
	{
		name: "02-rss-noguid",
		xml: `<?xml version="1.0"?>
<rss version="2.0"><channel><title>No Guid</title><link>https://noguid.example/</link><description>d</description>
<item><title>Has link and title</title><link>https://noguid.example/a</link><description>a</description></item>
<item><title>Same title other link</title><link>https://noguid.example/b</link><description>b</description></item>
<item><link>https://noguid.example/c</link><description>only link</description></item>
<item><title>Only title</title><description>only title</description></item>
<item><description>nothing but a description</description></item>
<item><title>日本語のタイトル</title><link>https://noguid.example/日本語</link></item>
<item><title>  padded  </title><link> https://noguid.example/pad </link></item>
</channel></rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "a9aec41c17411439333e6f879126796",
			Title:       ptr("No Guid"),
			Description: ptr("d"),
			Links:       []string{"https://noguid.example/"},
			Entries: []vectorEntry{
				{
					GUID:    "31636a81bad6e82630109f60b949cea7",
					Title:   ptr("Has link and title"),
					Link:    ptr("https://noguid.example/a"),
					Links:   []string{"https://noguid.example/a"},
					Content: ptr("a"),
					Summary: ptr("a"),
				},
				{
					GUID:    "1189deec9f38ead2afa105da89a19dd5",
					Title:   ptr("Same title other link"),
					Link:    ptr("https://noguid.example/b"),
					Links:   []string{"https://noguid.example/b"},
					Content: ptr("b"),
					Summary: ptr("b"),
				},
				{
					GUID:    "6f9dc5106c34ff0c4f23fa2800762b56",
					Link:    ptr("https://noguid.example/c"),
					Links:   []string{"https://noguid.example/c"},
					Content: ptr("only link"),
					Summary: ptr("only link"),
				},
				{
					GUID:    "d31d3b3e-eeae-41a4-9cbd-cceb672881a1",
					Title:   ptr("Only title"),
					Links:   []string{},
					Content: ptr("only title"),
					Summary: ptr("only title"),
				},
				{
					GUID:    "b87c4fd1-52ea-4c41-b7ce-de6257d368b3",
					Links:   []string{},
					Content: ptr("nothing but a description"),
					Summary: ptr("nothing but a description"),
				},
				{
					GUID:  "45115ed896411f9de954f30ad9d7718c",
					Title: ptr("日本語のタイトル"),
					Link:  ptr("https://noguid.example/%E6%97%A5%E6%9C%AC%E8%AA%9E"),
					Links: []string{"https://noguid.example/%E6%97%A5%E6%9C%AC%E8%AA%9E"},
				},
				{
					GUID:  "e57ea37d53f5ea5a207900e444f93edd",
					Title: ptr("padded"),
					Link:  ptr("https://noguid.example/pad"),
					Links: []string{"https://noguid.example/pad"},
				},
			},
		},
	},
	{
		name: "03-rss-guid-whitespace",
		xml: `<?xml version="1.0"?>
<rss version="2.0"><channel><title>Guid Space</title><link>https://gs.example/</link><description>d</description>
<item><title>T1</title><guid>  spaced-guid  </guid><link>https://gs.example/1</link></item>
<item><title>T2</title><guid isPermaLink="false">
 multi
 line </guid><link>https://gs.example/2</link></item>
<item><title>T3</title><guid></guid><link>https://gs.example/3</link></item>
<item><title>T4</title><guid>dup</guid><link>https://gs.example/4</link></item>
<item><title>T5</title><guid>dup</guid><link>https://gs.example/5</link></item>
</channel></rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "76e23b1998c1f46f1595f76032bec9bf",
			Title:       ptr("Guid Space"),
			Description: ptr("d"),
			Links:       []string{"https://gs.example/"},
			Entries: []vectorEntry{
				{
					GUID:  "spaced-guid",
					Title: ptr("T1"),
					Link:  ptr("https://gs.example/1"),
					Links: []string{"https://gs.example/1"},
				},
				{
					GUID:  "multi\n line",
					Title: ptr("T2"),
					Link:  ptr("https://gs.example/2"),
					Links: []string{"https://gs.example/2"},
				},
				{
					GUID:  "f5f93ed0adc09754f7ab5d807b51c92d",
					Title: ptr("T3"),
					Link:  ptr("https://gs.example/3"),
					Links: []string{"https://gs.example/3"},
				},
				{
					GUID:  "dup",
					Title: ptr("T4"),
					Link:  ptr("https://gs.example/4"),
					Links: []string{"https://gs.example/4"},
				},
				{
					GUID:  "dup",
					Title: ptr("T5"),
					Link:  ptr("https://gs.example/5"),
					Links: []string{"https://gs.example/5"},
				},
			},
		},
	},
	{
		name: "04-atom-basic",
		xml: `<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
<title>Atom Blog</title>
<subtitle>An atom blog</subtitle>
<link href="https://atom.example/" rel="alternate"/>
<link href="https://atom.example/feed.atom" rel="self"/>
<updated>2024-03-05T10:20:30Z</updated>
<id>urn:uuid:feed-1</id>
<icon>https://atom.example/icon.png</icon>
<logo>https://atom.example/logo.png</logo>
<entry>
<title>Atom one</title>
<link href="https://atom.example/1"/>
<id>urn:uuid:entry-1</id>
<published>2024-03-04T08:00:00+08:00</published>
<updated>2024-03-04T09:00:00Z</updated>
<summary>Atom summary</summary>
<content type="html">&lt;p&gt;Atom &lt;b&gt;html&lt;/b&gt;&lt;/p&gt;</content>
<author><name>Dana</name><email>dana@example.com</email></author>
</entry>
<entry>
<title type="text">Atom two</title>
<link href="https://atom.example/2" rel="alternate"/>
<id>  urn:uuid:entry-2  </id>
<updated>2024-03-03T09:00:00.123Z</updated>
<summary type="html">&lt;i&gt;only summary&lt;/i&gt;</summary>
</entry>
<entry>
<title>Atom three</title>
<link href="https://atom.example/3"/>
<id>urn:uuid:entry-3</id>
<content type="text">plain text content</content>
<author><name>Erin</name></author>
<author><name>Frank</name></author>
</entry>
</feed>
`,
		want: vectorFeed{
			FeedType:    "Atom",
			ID:          "urn:uuid:feed-1",
			Title:       ptr("Atom Blog"),
			Description: ptr("An atom blog"),
			Updated:     ptr("2024-03-05T10:20:30Z"),
			Links:       []string{"https://atom.example/", "https://atom.example/feed.atom"},
			Icon:        ptr("https://atom.example/icon.png"),
			Logo:        ptr("https://atom.example/logo.png"),
			Entries: []vectorEntry{
				{
					GUID:      "urn:uuid:entry-1",
					Title:     ptr("Atom one"),
					Link:      ptr("https://atom.example/1"),
					Links:     []string{"https://atom.example/1"},
					Content:   ptr("<p>Atom <b>html</b></p>"),
					Summary:   ptr("Atom summary"),
					Author:    ptr("Dana"),
					Published: ptr("2024-03-04T00:00:00Z"),
					Updated:   ptr("2024-03-04T09:00:00Z"),
				},
				{
					GUID:    "  urn:uuid:entry-2  ",
					Title:   ptr("Atom two"),
					Link:    ptr("https://atom.example/2"),
					Links:   []string{"https://atom.example/2"},
					Content: ptr("<i>only summary</i>"),
					Summary: ptr("<i>only summary</i>"),
					Updated: ptr("2024-03-03T09:00:00.123Z"),
				},
				{
					GUID:    "urn:uuid:entry-3",
					Title:   ptr("Atom three"),
					Link:    ptr("https://atom.example/3"),
					Links:   []string{"https://atom.example/3"},
					Content: ptr("plain text content"),
					Author:  ptr("Erin"),
				},
			},
		},
	},
	{
		name: "05-atom-noid",
		xml: `<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>No ids</title><updated>2024-01-01T00:00:00Z</updated>
<entry><title>With link</title><link href="https://noid.example/1"/><updated>2024-01-01T00:00:00Z</updated></entry>
<entry><title>With link again</title><link href="https://noid.example/2"/></entry>
<entry><title>No link</title></entry>
<entry><link href="https://noid.example/4"/></entry>
<entry></entry>
</feed>
`,
		want: vectorFeed{
			FeedType: "Atom",
			ID:       "e3155fa5-1cd8-426e-a290-421d4e377434",
			Title:    ptr("No ids"),
			Updated:  ptr("2024-01-01T00:00:00Z"),
			Links:    []string{},
			Entries: []vectorEntry{
				{
					GUID:    "ae17340a9b80582b773071bfa569537b",
					Title:   ptr("With link"),
					Link:    ptr("https://noid.example/1"),
					Links:   []string{"https://noid.example/1"},
					Updated: ptr("2024-01-01T00:00:00Z"),
				},
				{
					GUID:  "378b9bc8e34ec69cc551c85de116e105",
					Title: ptr("With link again"),
					Link:  ptr("https://noid.example/2"),
					Links: []string{"https://noid.example/2"},
				},
				{
					GUID:  "3f1813e0-d121-407c-aa8e-9bc5c200d2c6",
					Title: ptr("No link"),
					Links: []string{},
				},
				{
					GUID:  "41f6e83826822230e060e035f3ab2c4",
					Link:  ptr("https://noid.example/4"),
					Links: []string{"https://noid.example/4"},
				},
				{
					GUID:  "471b7f7c-2437-413f-af07-3036b42c57cf",
					Links: []string{},
				},
			},
		},
	},
	{
		name: "06-atom-multiple-links",
		xml: `<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Links</title><id>f</id><updated>2024-01-01T00:00:00Z</updated>
<link rel="self" href="https://links.example/feed"/>
<link rel="alternate" type="text/html" href="https://links.example/"/>
<entry><title>E1</title><id>e1</id><updated>2024-01-01T00:00:00Z</updated>
<link rel="enclosure" href="https://links.example/e1.mp3" type="audio/mpeg"/>
<link rel="alternate" href="https://links.example/e1"/>
<link rel="replies" href="https://links.example/e1#comments"/>
</entry>
<entry><title>E2</title><id>e2</id><updated>2024-01-01T00:00:00Z</updated>
<link href="https://links.example/e2"/><link href="https://links.example/e2"/>
</entry>
</feed>
`,
		want: vectorFeed{
			FeedType: "Atom",
			ID:       "f",
			Title:    ptr("Links"),
			Updated:  ptr("2024-01-01T00:00:00Z"),
			Links:    []string{"https://links.example/feed", "https://links.example/"},
			Entries: []vectorEntry{
				{
					GUID:    "e1",
					Title:   ptr("E1"),
					Link:    ptr("https://links.example/e1.mp3"),
					Links:   []string{"https://links.example/e1.mp3", "https://links.example/e1", "https://links.example/e1#comments"},
					Updated: ptr("2024-01-01T00:00:00Z"),
				},
				{
					GUID:    "e2",
					Title:   ptr("E2"),
					Link:    ptr("https://links.example/e2"),
					Links:   []string{"https://links.example/e2", "https://links.example/e2"},
					Updated: ptr("2024-01-01T00:00:00Z"),
				},
			},
		},
	},
	{
		name: "07-rdf-basic",
		xml: `<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:content="http://purl.org/rss/1.0/modules/content/">
<channel rdf:about="https://rdf.example/"><title>RDF Feed</title><link>https://rdf.example/</link><description>rdf desc</description></channel>
<item rdf:about="https://rdf.example/1"><title>RDF one</title><link>https://rdf.example/1</link><description>rdf summary</description><dc:creator>Gina</dc:creator><dc:date>2024-03-05T10:20:30+00:00</dc:date></item>
<item rdf:about="https://rdf.example/2"><title>RDF two</title><link>https://rdf.example/2</link><content:encoded>&lt;p&gt;rdf content&lt;/p&gt;</content:encoded></item>
</rdf:RDF>
`,
		want: vectorFeed{
			FeedType:    "RSS1",
			ID:          "bce6453ab5fa869e64005b09430d32fd",
			Title:       ptr("RDF Feed"),
			Description: ptr("rdf desc"),
			Links:       []string{"https://rdf.example/"},
			Entries: []vectorEntry{
				{
					GUID:      "df937f558d2ee71cd8fc6a431b2a32b0",
					Title:     ptr("RDF one"),
					Link:      ptr("https://rdf.example/1"),
					Links:     []string{"https://rdf.example/1"},
					Content:   ptr("rdf summary"),
					Summary:   ptr("rdf summary"),
					Author:    ptr("Gina"),
					Published: ptr("2024-03-05T10:20:30Z"),
				},
				{
					GUID:    "d0305c7c82efa04cb27e359b6e24adbb",
					Title:   ptr("RDF two"),
					Link:    ptr("https://rdf.example/2"),
					Links:   []string{"https://rdf.example/2"},
					Content: ptr("<p>rdf content</p>"),
				},
			},
		},
	},
	{
		name: "08-rss-chinese-dates",
		xml: `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>中文</title><link>https://zh.example/</link><description>中文描述</description>
<lastBuildDate>週二, 05 三月 2024 10:20:30 +0800</lastBuildDate>
<item><title>一</title><guid>zh-1</guid><pubDate>週二, 05 三月 2024 10:20:30 +0800</pubDate></item>
<item><title>二</title><guid>zh-2</guid><pubDate>05 三月 2024 10:20:30</pubDate></item>
<item><title>三</title><guid>zh-3</guid><pubDate>2024-03-05 10:20:30</pubDate></item>
<item><title>四</title><guid>zh-4</guid><pubDate>2024-03-05</pubDate></item>
<item><title>五</title><guid>zh-5</guid><pubDate>not a date</pubDate></item>
<item><title>六</title><guid>zh-6</guid><pubDate>Tue, 05 Mar 2024 10:20:30 +08:00</pubDate></item>
<item><title>七</title><guid>zh-7</guid><pubDate>Tue, 5 Mar 2024 10:20 GMT</pubDate></item>
</channel></rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "49d288c6f1b1d093269f3696a673ee4c",
			Title:       ptr("中文"),
			Description: ptr("中文描述"),
			Updated:     ptr("2024-03-05T02:20:30Z"),
			Links:       []string{"https://zh.example/"},
			Entries: []vectorEntry{
				{
					GUID:      "zh-1",
					Title:     ptr("一"),
					Links:     []string{},
					Published: ptr("2024-03-05T02:20:30Z"),
					Updated:   ptr("2024-03-05T02:20:30Z"),
				},
				{
					GUID:      "zh-2",
					Title:     ptr("二"),
					Links:     []string{},
					Published: ptr("2024-03-05T10:20:30Z"),
					Updated:   ptr("2024-03-05T10:20:30Z"),
				},
				{
					GUID:      "zh-3",
					Title:     ptr("三"),
					Links:     []string{},
					Published: ptr("2024-03-05T10:20:30Z"),
					Updated:   ptr("2024-03-05T10:20:30Z"),
				},
				{
					GUID:      "zh-4",
					Title:     ptr("四"),
					Links:     []string{},
					Published: ptr("2024-03-05T00:00:00Z"),
					Updated:   ptr("2024-03-05T00:00:00Z"),
				},
				{
					GUID:  "zh-5",
					Title: ptr("五"),
					Links: []string{},
				},
				{
					GUID:      "zh-6",
					Title:     ptr("六"),
					Links:     []string{},
					Published: ptr("2024-03-05T02:20:30Z"),
					Updated:   ptr("2024-03-05T02:20:30Z"),
				},
				{
					GUID:      "zh-7",
					Title:     ptr("七"),
					Links:     []string{},
					Published: ptr("2024-03-05T10:20:00Z"),
					Updated:   ptr("2024-03-05T10:20:00Z"),
				},
			},
		},
	},
	{
		name: "09-rss-no-dates",
		xml: `<?xml version="1.0"?>
<rss version="2.0"><channel><title>Dateless</title><link>https://nodate.example/</link><description>d</description>
<item><title>A</title><guid>a</guid></item>
<item><title>B</title><guid>b</guid></item>
</channel></rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "399b96515ca98c3b4b93937c6a64b156",
			Title:       ptr("Dateless"),
			Description: ptr("d"),
			Links:       []string{"https://nodate.example/"},
			Entries: []vectorEntry{
				{
					GUID:  "a",
					Title: ptr("A"),
					Links: []string{},
				},
				{
					GUID:  "b",
					Title: ptr("B"),
					Links: []string{},
				},
			},
		},
	},
	{
		name: "10-rss-cdata-entities",
		xml: `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>Tom &amp; Jerry's &lt;Show&gt;</title><link>https://ent.example/</link><description><![CDATA[<b>bold</b> desc]]></description>
<item><title><![CDATA[CDATA & title]]></title><guid>e1</guid><description>&lt;p&gt;escaped &amp;amp; html&lt;/p&gt;</description></item>
<item><title>Caf&#233; &#x4e2d; &quot;q&quot;</title><guid>e2</guid><description><![CDATA[<p>cdata <a href="https://ent.example/x?a=1&b=2">link</a></p>]]></description></item>
</channel></rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "e15a3fa06db6a34ac0b2048628ee626e",
			Title:       ptr("Tom & Jerry's <Show>"),
			Description: ptr("<b>bold</b> desc"),
			Links:       []string{"https://ent.example/"},
			Entries: []vectorEntry{
				{
					GUID:    "e1",
					Title:   ptr("CDATA & title"),
					Links:   []string{},
					Content: ptr("<p>escaped &amp; html</p>"),
					Summary: ptr("<p>escaped &amp; html</p>"),
				},
				{
					GUID:    "e2",
					Title:   ptr("Café 中 \"q\""),
					Links:   []string{},
					Content: ptr("<p>cdata <a href=\"https://ent.example/x?a=1&b=2\">link</a></p>"),
					Summary: ptr("<p>cdata <a href=\"https://ent.example/x?a=1&b=2\">link</a></p>"),
				},
			},
		},
	},
	{
		name: "11-atom-xhtml-content",
		xml: `<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:xhtml="http://www.w3.org/1999/xhtml"><title type="html">&lt;b&gt;Html&lt;/b&gt; title</title><id>x</id><updated>2024-01-01T00:00:00Z</updated>
<entry><title type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml">XHTML <b>title</b></div></title><id>x1</id><updated>2024-01-01T00:00:00Z</updated>
<content type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml"><p>Para <em>one</em></p></div></content></entry>
<entry><title>Src content</title><id>x2</id><updated>2024-01-02T00:00:00Z</updated><content type="text/html" src="https://x.example/body"/></entry>
</feed>
`,
		want: vectorFeed{
			FeedType: "Atom",
			ID:       "x",
			Title:    ptr("<b>Html</b> title"),
			Updated:  ptr("2024-01-01T00:00:00Z"),
			Links:    []string{},
			Entries: []vectorEntry{
				{
					GUID:    "x1",
					Title:   ptr("XHTML <b>title</b>"),
					Links:   []string{},
					Content: ptr("<p>Para <em>one</em></p>"),
					Updated: ptr("2024-01-01T00:00:00Z"),
				},
				{
					GUID:    "x2",
					Title:   ptr("Src content"),
					Links:   []string{},
					Updated: ptr("2024-01-02T00:00:00Z"),
				},
			},
		},
	},
	{
		name: "12-rss-author-forms",
		xml: `<?xml version="1.0"?>
<rss version="2.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"><channel><title>Authors</title><link>https://au.example/</link><description>d</description>
<item><title>1</title><guid>a1</guid><author>a@b.com (Name One)</author></item>
<item><title>2</title><guid>a2</guid><author>Plain Name</author></item>
<item><title>3</title><guid>a3</guid><dc:creator>Creator Name</dc:creator><author>other@b.com (Other)</author></item>
<item><title>4</title><guid>a4</guid><itunes:author>Itunes Name</itunes:author></item>
<item><title>5</title><guid>a5</guid><author></author></item>
</channel></rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "77ca862e040bf2f99b7c5c5edea59460",
			Title:       ptr("Authors"),
			Description: ptr("d"),
			Links:       []string{"https://au.example/"},
			Entries: []vectorEntry{
				{
					GUID:   "a1",
					Title:  ptr("1"),
					Links:  []string{},
					Author: ptr("(Name One)"),
				},
				{
					GUID:   "a2",
					Title:  ptr("2"),
					Links:  []string{},
					Author: ptr("Plain Name"),
				},
				{
					GUID:   "a3",
					Title:  ptr("3"),
					Links:  []string{},
					Author: ptr("Creator Name"),
				},
				{
					GUID:  "a4",
					Title: ptr("4"),
					Links: []string{},
				},
				{
					GUID:  "a5",
					Title: ptr("5"),
					Links: []string{},
				},
			},
		},
	},
	{
		name: "13-rss-enclosure-media",
		xml: `<?xml version="1.0"?>
<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel><title>Podcast</title><link>https://pod.example/</link><description>d</description>
<item><title>Ep1</title><guid>ep1</guid><link>https://pod.example/ep1</link><comments>https://pod.example/ep1#c</comments>
<enclosure url="https://pod.example/ep1.mp3" length="123" type="audio/mpeg"/>
<media:content url="https://pod.example/ep1.jpg" type="image/jpeg"><media:description>media desc</media:description></media:content>
<media:thumbnail url="https://pod.example/t.jpg"/></item>
<item><title>Ep2</title><guid>ep2</guid><comments>https://pod.example/ep2#c</comments></item>
<item><title>Ep3</title><guid>ep3</guid><link>https://pod.example/ep3</link><link>https://pod.example/ep3-b</link></item>
</channel></rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "40c55f80ea708b81b9df7cf98991f929",
			Title:       ptr("Podcast"),
			Description: ptr("d"),
			Links:       []string{"https://pod.example/"},
			Entries: []vectorEntry{
				{
					GUID:  "ep1",
					Title: ptr("Ep1"),
					Link:  ptr("https://pod.example/ep1"),
					Links: []string{"https://pod.example/ep1", "https://pod.example/ep1#c"},
				},
				{
					GUID:  "ep2",
					Title: ptr("Ep2"),
					Link:  ptr("https://pod.example/ep2#c"),
					Links: []string{"https://pod.example/ep2#c"},
				},
				{
					GUID:  "ep3",
					Title: ptr("Ep3"),
					Link:  ptr("https://pod.example/ep3"),
					Links: []string{"https://pod.example/ep3", "https://pod.example/ep3-b"},
				},
			},
		},
	},
	{
		name: "14-atom-updated-only",
		xml: `<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Updated only</title><id>u</id><updated>2024-05-06T07:08:09-07:00</updated>
<entry><title>U1</title><id>u1</id><link href="https://u.example/1"/><updated>2024-05-06T07:08:09-07:00</updated></entry>
<entry><title>U2</title><id>u2</id><link href="https://u.example/2"/><published>2024-05-01T00:00:00Z</published></entry>
<entry><title>U3</title><id>u3</id><link href="https://u.example/3"/><published>bad date</published><updated>2024-05-02T00:00:00Z</updated></entry>
</feed>
`,
		want: vectorFeed{
			FeedType: "Atom",
			ID:       "u",
			Title:    ptr("Updated only"),
			Updated:  ptr("2024-05-06T14:08:09Z"),
			Links:    []string{},
			Entries: []vectorEntry{
				{
					GUID:    "u1",
					Title:   ptr("U1"),
					Link:    ptr("https://u.example/1"),
					Links:   []string{"https://u.example/1"},
					Updated: ptr("2024-05-06T14:08:09Z"),
				},
				{
					GUID:      "u2",
					Title:     ptr("U2"),
					Link:      ptr("https://u.example/2"),
					Links:     []string{"https://u.example/2"},
					Published: ptr("2024-05-01T00:00:00Z"),
				},
				{
					GUID:    "u3",
					Title:   ptr("U3"),
					Link:    ptr("https://u.example/3"),
					Links:   []string{"https://u.example/3"},
					Updated: ptr("2024-05-02T00:00:00Z"),
				},
			},
		},
	},
	{
		name: "15-rss-no-title-no-channel-link",
		xml: `<?xml version="1.0"?>
<rss version="2.0"><channel><description>No title</description>
<item><guid>n1</guid><description>desc only</description></item>
<item><title></title><guid>n2</guid></item>
</channel></rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "bd8d3fe8-e691-4aa9-955a-3348992231f1",
			Description: ptr("No title"),
			Links:       []string{},
			Entries: []vectorEntry{
				{
					GUID:    "n1",
					Links:   []string{},
					Content: ptr("desc only"),
					Summary: ptr("desc only"),
				},
				{
					GUID:  "n2",
					Links: []string{},
				},
			},
		},
	},
	{
		name: "16-rss-source-fragment",
		xml: `<?xml version="1.0"?>
<rss version="2.0"><channel><title>Rel</title><link>/</link><description>d</description>
<item><title>Rel link</title><link>/relative/post</link><guid>r1</guid></item>
<item><title>Query</title><link>https://q.example/p?a=1&amp;b=2#frag</link><guid>r2</guid></item>
</channel></rss>
`,
		want: vectorFeed{
			FeedType:    "RSS2",
			ID:          "eb129f690ef9881b53b1ec17a8c54277",
			Title:       ptr("Rel"),
			Description: ptr("d"),
			Links:       []string{"/"},
			Entries: []vectorEntry{
				{
					GUID:  "r1",
					Title: ptr("Rel link"),
					Link:  ptr("/relative/post"),
					Links: []string{"/relative/post"},
				},
				{
					GUID:  "r2",
					Title: ptr("Query"),
					Link:  ptr("https://q.example/p?a=1&b=2#frag"),
					Links: []string{"https://q.example/p?a=1&b=2#frag"},
				},
			},
		},
	},
	{
		name: "17-invalid",
		xml: `<html><body>not a feed</body></html>
`,
		want: vectorFeed{
			Error: "unable to parse feed: no root element",
		},
	},
}
