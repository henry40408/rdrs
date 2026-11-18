package main

// feed is a feed to seed, its real favicon, and the entries it shows.
type feed struct {
	url     string
	title   string
	icon    string
	entries []entry
}

type entry struct {
	title   string
	content string
}

type category struct {
	name  string
	feeds []feed
}

// demoContent is realistic content, inspired by NetNewsWire's default feeds.
var demoContent = []category{
	{"Apple & Tech", []feed{
		{"https://daringfireball.net/feeds/json", "Daring Fireball", "https://daringfireball.net/graphics/favicon-64.png", []entry{
			{"The M5 Ultra and the Future of Mac Pro", "<p>Apple's latest M5 Ultra chip represents a significant leap in performance for creative professionals. The new Mac Pro, powered by this chip, offers unprecedented capabilities for video editing, 3D rendering, and machine learning workloads. The unified memory architecture now supports up to 512GB, making it a true workstation-class machine.</p>"},
			{"Safari 20 Ships With Vertical Tabs", "<p>After years of requests, Safari finally adds vertical tab support. The implementation is clean and native-feeling, with tabs arranged in a sidebar that can be toggled with a keyboard shortcut. It integrates beautifully with Tab Groups and iCloud sync.</p>"},
		}},
		{"https://mjtsai.com/blog/feed/", "Michael Tsai", "https://mjtsai.com/favicon.ico", []entry{
			{"Swift 6.2 Concurrency Changes", "<p>Swift 6.2 brings several refinements to the concurrency model. The most notable change is the introduction of region-based isolation, which makes it easier to reason about data race safety without sacrificing ergonomics.</p>"},
			{"App Store Review Times and Transparency", "<p>A collection of developer experiences with recent App Store review processes. Several developers report improved review times, while others note inconsistencies in guideline enforcement across different reviewers.</p>"},
		}},
		{"https://sixcolors.com/feed/", "Six Colors", "https://sixcolors.com/favicon.ico", []entry{
			{"WWDC 2026: What to Expect", "<p>With WWDC just around the corner, here's our comprehensive preview of what Apple might announce. From iOS 20 to macOS 17, visionOS 3, and potential hardware surprises, this year's developer conference promises to be packed with announcements.</p>"},
		}},
	}},
	{"Indie & Web", []feed{
		{"https://inessential.com/feed.json", "inessential", "https://inessential.com/favicon.ico", []entry{
			{"On Building RSS Readers in 2026", "<p>RSS is having a quiet renaissance. More people are turning to feed readers as an antidote to algorithmic timelines. The protocol's simplicity is its greatest strength — it does one thing well, and it respects the reader's attention.</p>"},
			{"Why I Still Write a Blog", "<p>In an era of social media and short-form content, maintaining a blog feels almost rebellious. But there's something deeply satisfying about owning your words, publishing on your own schedule, and building a body of work over decades.</p>"},
		}},
		{"https://jvns.ca/atom.xml", "Julia Evans", "https://jvns.ca/favicon.ico", []entry{
			{"A Little Bit About HTTP Caching", "<p>HTTP caching is one of those things that seems simple on the surface but has a surprising amount of depth. Let's look at Cache-Control headers, ETags, conditional requests, and how browsers actually decide whether to use a cached response.</p>"},
			{"How Git Stores Objects", "<p>Ever wondered what's actually inside the .git directory? Let's explore how Git stores commits, trees, and blobs as content-addressed objects, and why this design makes operations like branching and merging so fast.</p>"},
		}},
		{"https://kottke.org/feed/json", "Jason Kottke", "https://kottke.org/favicon.ico", []entry{
			{"The Web We Lost and the Web We Found", "<p>A reflection on how the open web has evolved over the past two decades. While we've lost some of the early web's anarchic creativity, new tools and protocols are enabling a different kind of independence.</p>"},
		}},
		{"https://netnewswire.blog/feed.json", "NetNewsWire Blog", "https://netnewswire.blog/favicon.ico", []entry{
			{"NetNewsWire 7: What's New", "<p>The latest release of NetNewsWire brings a refreshed design, improved sync performance, and better support for feed discovery. We've also added new keyboard shortcuts and enhanced the reading experience.</p>"},
		}},
	}},
}
