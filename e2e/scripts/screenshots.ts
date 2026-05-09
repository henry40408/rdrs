// Screenshot generator for README
// Run: cd e2e && npx playwright test screenshots.spec.ts
import { test, expect } from "../fixtures/rdrs.js";
import path from "path";

const SCREENSHOT_DIR = path.resolve(__dirname, "..", "..", "screenshots");

// Favicon URLs for each feed (domain → favicon path)
const FAVICON_URLS: Record<string, string> = {
  "https://daringfireball.net/feeds/json": "https://daringfireball.net/graphics/favicon-64.png",
  "https://mjtsai.com/blog/feed/": "https://mjtsai.com/favicon.ico",
  "https://sixcolors.com/feed/": "https://sixcolors.com/favicon.ico",
  "https://inessential.com/feed.json": "https://inessential.com/favicon.ico",
  "https://jvns.ca/atom.xml": "https://jvns.ca/favicon.ico",
  "https://kottke.org/feed/json": "https://kottke.org/favicon.ico",
  "https://netnewswire.blog/feed.json": "https://netnewswire.blog/favicon.ico",
};

/** Fetch a favicon, returning { data, contentType } or null on failure. */
async function fetchFavicon(url: string): Promise<{ data: Buffer; contentType: string } | null> {
  try {
    const res = await fetch(url, { signal: AbortSignal.timeout(5000) });
    if (!res.ok) return null;
    const contentType = res.headers.get("content-type") || "image/x-icon";
    const data = Buffer.from(await res.arrayBuffer());
    if (data.length === 0 || data.length > 256 * 1024) return null;
    return { data, contentType };
  } catch {
    return null;
  }
}

// Realistic entries inspired by NetNewsWire's default feeds
const SEED_FEEDS = [
  {
    category: "Apple & Tech",
    feeds: [
      {
        url: "https://daringfireball.net/feeds/json",
        title: "Daring Fireball",
        entries: [
          {
            title: "The M5 Ultra and the Future of Mac Pro",
            content: "<p>Apple's latest M5 Ultra chip represents a significant leap in performance for creative professionals. The new Mac Pro, powered by this chip, offers unprecedented capabilities for video editing, 3D rendering, and machine learning workloads. The unified memory architecture now supports up to 512GB, making it a true workstation-class machine.</p>",
          },
          {
            title: "Safari 20 Ships With Vertical Tabs",
            content: "<p>After years of requests, Safari finally adds vertical tab support. The implementation is clean and native-feeling, with tabs arranged in a sidebar that can be toggled with a keyboard shortcut. It integrates beautifully with Tab Groups and iCloud sync.</p>",
          },
        ],
      },
      {
        url: "https://mjtsai.com/blog/feed/",
        title: "Michael Tsai",
        entries: [
          {
            title: "Swift 6.2 Concurrency Changes",
            content: "<p>Swift 6.2 brings several refinements to the concurrency model. The most notable change is the introduction of region-based isolation, which makes it easier to reason about data race safety without sacrificing ergonomics.</p>",
          },
          {
            title: "App Store Review Times and Transparency",
            content: "<p>A collection of developer experiences with recent App Store review processes. Several developers report improved review times, while others note inconsistencies in guideline enforcement across different reviewers.</p>",
          },
        ],
      },
      {
        url: "https://sixcolors.com/feed/",
        title: "Six Colors",
        entries: [
          {
            title: "WWDC 2026: What to Expect",
            content: "<p>With WWDC just around the corner, here's our comprehensive preview of what Apple might announce. From iOS 20 to macOS 17, visionOS 3, and potential hardware surprises, this year's developer conference promises to be packed with announcements.</p>",
          },
        ],
      },
    ],
  },
  {
    category: "Indie & Web",
    feeds: [
      {
        url: "https://inessential.com/feed.json",
        title: "inessential",
        entries: [
          {
            title: "On Building RSS Readers in 2026",
            content: "<p>RSS is having a quiet renaissance. More people are turning to feed readers as an antidote to algorithmic timelines. The protocol's simplicity is its greatest strength — it does one thing well, and it respects the reader's attention.</p>",
          },
          {
            title: "Why I Still Write a Blog",
            content: "<p>In an era of social media and short-form content, maintaining a blog feels almost rebellious. But there's something deeply satisfying about owning your words, publishing on your own schedule, and building a body of work over decades.</p>",
          },
        ],
      },
      {
        url: "https://jvns.ca/atom.xml",
        title: "Julia Evans",
        entries: [
          {
            title: "A Little Bit About HTTP Caching",
            content: "<p>HTTP caching is one of those things that seems simple on the surface but has a surprising amount of depth. Let's look at Cache-Control headers, ETags, conditional requests, and how browsers actually decide whether to use a cached response.</p>",
          },
          {
            title: "How Git Stores Objects",
            content: "<p>Ever wondered what's actually inside the .git directory? Let's explore how Git stores commits, trees, and blobs as content-addressed objects, and why this design makes operations like branching and merging so fast.</p>",
          },
        ],
      },
      {
        url: "https://kottke.org/feed/json",
        title: "Jason Kottke",
        entries: [
          {
            title: "The Web We Lost and the Web We Found",
            content: "<p>A reflection on how the open web has evolved over the past two decades. While we've lost some of the early web's anarchic creativity, new tools and protocols are enabling a different kind of independence.</p>",
          },
        ],
      },
      {
        url: "https://netnewswire.blog/feed.json",
        title: "NetNewsWire Blog",
        entries: [
          {
            title: "NetNewsWire 7: What's New",
            content: "<p>The latest release of NetNewsWire brings a refreshed design, improved sync performance, and better support for feed discovery. We've also added new keyboard shortcuts and enhanced the reading experience.</p>",
          },
        ],
      },
    ],
  },
];

test.use({ viewport: { width: 1920, height: 1080 } });

test.describe("Screenshots", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("demouser", "password123");
    const userId = seed.getUserId("demouser");

    // Fetch all favicons in parallel
    const faviconResults = await Promise.all(
      Object.entries(FAVICON_URLS).map(async ([feedUrl, iconUrl]) => ({
        feedUrl,
        iconUrl,
        result: await fetchFavicon(iconUrl),
      }))
    );
    const faviconMap = new Map(
      faviconResults
        .filter((r) => r.result !== null)
        .map((r) => [r.feedUrl, { ...r.result!, sourceUrl: r.iconUrl }])
    );

    let hourOffset = 1;
    for (const cat of SEED_FEEDS) {
      const categoryId = seed.createCategory(userId, cat.category);
      for (const feed of cat.feeds) {
        const feedId = seed.createFeed(categoryId, feed.url, feed.title);

        // Insert favicon if fetched successfully
        const icon = faviconMap.get(feed.url);
        if (icon) {
          seed.insertIcon(feedId, icon.data, icon.contentType, icon.sourceUrl);
        }

        const entries = feed.entries.map((e, i) => ({
          feedId,
          guid: `${feed.url}/entry-${i}`,
          title: e.title,
          link: `${feed.url.replace(/\/feed.*/, "")}/${e.title.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`,
          content: e.content,
          publishedOffset: `-${hourOffset++} hours`,
        }));
        seed.insertEntries(entries);
      }
    }
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("demouser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();

    // Wait for all feed icons to finish loading (loaded or errored)
    await page.waitForFunction(() => {
      const icons = document.querySelectorAll<HTMLImageElement>(".feed-icon");
      return Array.from(icons).every((img) => img.complete);
    });
  });

  for (const theme of ["light", "dark"] as const) {
    test.describe(theme, () => {
      test.use({ colorScheme: theme });
      const suffix = theme === "dark" ? "-dark" : "";

      test(`unread list with reading pane (${theme})`, async ({ page }) => {
        await page.keyboard.press("j");
        await expect(page.locator(".reading-pane-title")).toBeVisible();

        await page.screenshot({
          path: path.join(SCREENSHOT_DIR, `unread-list${suffix}.png`),
          fullPage: false,
        });
      });

      test(`keyboard shortcuts popup (${theme})`, async ({ page }) => {
        await page.keyboard.press("?");
        await expect(page.locator("rdrs-kb-help").first()).toHaveClass(/visible/);
        await page.waitForTimeout(200);

        await page.screenshot({
          path: path.join(SCREENSHOT_DIR, `keyboard-shortcuts${suffix}.png`),
          fullPage: false,
        });
      });
    });
  }
});
