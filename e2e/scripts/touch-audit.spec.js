// Touch-target audit @ iPhone-SE width (375px).
// Walks every interactive element on the main pages, records rendered
// bounding boxes, and reports anything < 44px in either axis.
// Run: cd e2e && npx playwright test scripts/touch-audit.spec.js --project=chromium
import { test } from "../support/fixtures.js";
import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.resolve(__dirname, "touch-audit-report.json");
const MIN = 44;

test("touch-target audit @ 375px", async ({ page, api, seed, serverUrl, currentUser }) => {
  test.setTimeout(180_000);

  // ---- seed a realistic account ----
  await api.register(currentUser.username, currentUser.password);
  const userId = seed.getUserId(currentUser.username);
  const catId = seed.createCategory(userId, "Tech");
  const feedId = seed.createFeed(catId, "https://example.com/feed.xml", "Example Feed");
  seed.seedTestEntries(feedId, 12);
  seed.configureKagi(userId);
  seed.makeAdmin(userId);

  // ---- UI login ----
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);

  await page.setViewportSize({ width: 375, height: 667 });

  const PAGES = [
    { name: "Unread", url: "/" },
    { name: "All Entries", url: "/entries" },
    { name: "Starred", url: "/entries/starred" },
    { name: "Summarized", url: "/entries/summarized" },
    { name: "Feeds", url: "/feeds" },
    { name: "Feed edit", url: `/feeds/${feedId}/edit` },
    { name: "Categories", url: "/categories" },
    { name: "Import OPML", url: "/feeds/import" },
    { name: "Search", url: "/search?q=test" },
    { name: "User settings", url: "/user-settings" },
    { name: "App settings", url: "/settings" },
    { name: "Statistics", url: "/statistics" },
    { name: "Admin", url: "/admin" },
  ];

  const measure = (openSidebar) =>
    page.evaluate(
      async ({ MIN, openSidebar }) => {
        if (openSidebar) {
          document.querySelector(".sidebar-toggle")?.click();
          await new Promise((r) => setTimeout(r, 250));
        }
        const sel =
          'button, a[href], select, input:not([type=hidden]), textarea, label:has(input,select,textarea), [role="button"], summary';
        const out = [];
        for (const el of document.querySelectorAll(sel)) {
          const r = el.getBoundingClientRect();
          const cs = getComputedStyle(el);
          if (cs.display === "none" || cs.visibility === "hidden") continue;
          if (r.width === 0 || r.height === 0) continue;
          if (r.width >= MIN && r.height >= MIN) continue;
          const label =
            el.getAttribute("data-testid") ||
            el.getAttribute("aria-label") ||
            (el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 32);
          // checkbox/radio inside a label: the label IS the tap target
          const type = el.getAttribute("type") || "";
          const labelWrapped =
            (type === "checkbox" || type === "radio") &&
            !!el.closest("label");
          out.push({
            tag: el.tagName.toLowerCase(),
            type,
            cls: (el.className?.toString() || "").slice(0, 44),
            label,
            w: Math.round(r.width),
            h: Math.round(r.height),
            // inline text links are an accepted exemption
            inlineText:
              !!el.closest("p, .entry-item-meta, .reading-pane-article, .breadcrumb") ||
              labelWrapped,
          });
        }
        return out;
      },
      { MIN, openSidebar }
    );

  const report = [];
  for (const [i, p] of PAGES.entries()) {
    await page.goto(`${serverUrl}${p.url}`, { waitUntil: "networkidle" }).catch(() => {});
    const findings = await measure(false);
    report.push({ page: p.name, url: p.url, findings });
    // measure the off-canvas sidebar drawer once
    if (i === 0) {
      const sb = await measure(true);
      report.push({ page: "Sidebar drawer", url: p.url, findings: sb });
    }
  }

  fs.writeFileSync(OUT, JSON.stringify(report, null, 2));

  // console summary
  let total = 0,
    realGaps = 0;
  for (const r of report) {
    if (!r.findings.length) {
      console.log(`\n## ${r.page} (${r.url}) — OK (all >= ${MIN}px)`);
      continue;
    }
    console.log(`\n## ${r.page} (${r.url}) — ${r.findings.length} sub-${MIN}px`);
    for (const f of r.findings) {
      total++;
      if (!f.inlineText) realGaps++;
      console.log(
        `  ${String(f.w).padStart(3)}x${String(f.h).padStart(3)}  <${f.tag}${
          f.type ? ` type=${f.type}` : ""
        }> ${f.label}  [.${f.cls}]${f.inlineText ? "  (inline-text exempt)" : ""}`
      );
    }
  }
  console.log(`\n=== ${total} sub-${MIN}px hits; ${realGaps} non-inline (candidate gaps). Report: ${OUT} ===`);
});
