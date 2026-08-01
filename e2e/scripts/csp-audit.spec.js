// CSP audit — walks the app in a real browser and fails on any Content
// Security Policy violation.
//
// This exists because the Rust-side guard in `src/middleware/security_headers.rs`
// is a *static* scan: it greps `templates/` and `static/js/` for `style="`,
// `<style`, `on*=` handlers and inline `<script>` bodies. That catches the
// authoring mistakes, but it is blind to everything the policy actually
// governs at runtime — a stylesheet `@import` to another origin, a webfont from
// a CDN, an `img-src` the markup never mentions, markup assigned to
// `innerHTML` by a script, or a `<style>` element built inside a shadow root.
// Only a browser enforcing the header can see those.
//
// Run: cd e2e && npm run test:csp
import { test, expect } from "../support/fixtures.js";

// Collected in the page and drained after every navigation. `securitypolicyviolation`
// is preferred over scraping console text: it is structured, stable across
// Chromium versions, and fires for attribute-level violations (an inline
// `style=`) that carry no blocked URI to match on.
//
// Playwright injects this through CDP's addScriptToEvaluateOnNewDocument, which
// is exempt from the page's own CSP — so the collector cannot be silenced by
// the very policy it is measuring.
const COLLECTOR = () => {
  window.__cspViolations = [];
  document.addEventListener("securitypolicyviolation", (e) => {
    window.__cspViolations.push({
      directive: e.effectiveDirective || e.violatedDirective,
      blockedURI: e.blockedURI,
      sourceFile: e.sourceFile,
      line: e.lineNumber,
      sample: (e.sample || "").slice(0, 80),
    });
  });
};

test("no CSP violations across the app", async ({ page, api, seed, serverUrl, currentUser }) => {
  test.setTimeout(180_000);

  await page.addInitScript(COLLECTOR);

  const violations = [];
  let where = "startup";

  // Drains the page-side buffer into the run-wide list. Must be called after
  // every navigation, because the collector is re-injected per document and the
  // buffer starts empty again.
  const drain = async () => {
    const found = await page.evaluate(() => {
      const v = window.__cspViolations || [];
      window.__cspViolations = [];
      return v;
    });
    for (const v of found) violations.push({ where, ...v });
  };

  // Navigation waits on "domcontentloaded", never "networkidle": every logged-in
  // page holds an open SSE stream, so the network never goes idle and
  // "networkidle" would time out on every single page.
  const visit = async (label, url) => {
    where = label;
    await page.goto(`${serverUrl}${url}`, { waitUntil: "domcontentloaded" });
    // Let deferred module scripts run and any swap fragment settle; a violation
    // raised by a script that has not executed yet would otherwise be missed.
    await page.waitForTimeout(250);
    await drain();
  };

  // ---- logged-out surfaces ----
  await visit("Login", "/login");
  await visit("Setup", "/setup");

  // ---- seed an account with something to render ----
  await api.register(currentUser.username, currentUser.password);
  const userId = seed.getUserId(currentUser.username);
  const catId = seed.createCategory(userId, "Tech");
  const feedId = seed.createFeed(catId, "https://example.com/feed.xml", "Example Feed");
  seed.seedTestEntries(feedId, 12);
  seed.configureKagi(userId);
  seed.makeAdmin(userId);

  where = "Login (submit)";
  await page.goto(`${serverUrl}/login`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
  await drain();

  // ---- logged-in surfaces ----
  for (const [label, url] of [
    ["Unread", "/"],
    ["All entries", "/entries"],
    ["Starred", "/entries/starred"],
    ["Summarized", "/entries/summarized"],
    ["Feeds", "/feeds"],
    ["Feed edit", `/feeds/${feedId}/edit`],
    ["Categories", "/categories"],
    ["Import OPML", "/feeds/import"],
    ["Search", "/search?q=test"],
    ["User settings", "/user-settings"],
    ["App settings", "/settings"],
    // The statistics bars carry their geometry as `pct-N` classes rather than an
    // inline `style` — this page is the reason that scale exists.
    ["Statistics", "/statistics"],
    ["Admin", "/admin"],
  ]) {
    await visit(label, url);
  }

  // ---- runtime-injected markup, which the static scan cannot reach ----

  // The reading pane arrives as an HTML fragment swapped into the document by
  // script; its markup is parsed under the same policy as the page.
  where = "Reading pane (swap fragment)";
  await page.goto(`${serverUrl}/`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("entry-item").first().getByTestId("entry-title-link").click();
  await page.locator("#reading-pane:not(.reading-pane-empty)").waitFor({ state: "attached" });
  await page.waitForTimeout(250);
  await drain();

  // The keyboard-help overlay builds a shadow root. A <style> element inside a
  // shadow tree is still markup and still policed, which is why rdrs-kb-help
  // uses a constructable stylesheet instead.
  where = "Keyboard help overlay (shadow DOM)";
  await page.keyboard.press("?");
  await expect(page.getByTestId("kb-help")).toBeVisible();
  await page.waitForTimeout(250);
  await drain();
  await page.keyboard.press("Escape");

  // The off-canvas sidebar is only reachable at a narrow viewport.
  where = "Sidebar drawer (mobile)";
  await page.setViewportSize({ width: 375, height: 667 });
  await page.goto(`${serverUrl}/`, { waitUntil: "domcontentloaded" });
  await page.locator(".sidebar-toggle").click();
  await page.waitForTimeout(250);
  await drain();
  await page.setViewportSize({ width: 1280, height: 800 });

  // ---- verdict ----
  const describe = (v) =>
    `  [${v.where}] ${v.directive} blocked ${v.blockedURI || "(inline)"}` +
    `${v.sourceFile ? ` from ${v.sourceFile}:${v.line}` : ""}` +
    `${v.sample ? ` — sample: ${v.sample}` : ""}`;
  expect(violations.map(describe).join("\n"), "CSP violations detected").toBe("");

  // ---- positive control ----
  // An audit that reports zero findings is worthless unless the collector is
  // known to have been live. Plant a violation the policy must reject and
  // require it to surface; if this assertion fails, every clean result above is
  // meaningless rather than reassuring.
  where = "positive control";
  await page.evaluate(() => {
    document.body.insertAdjacentHTML("beforeend", '<div id="csp-control" style="color:red"></div>');
  });
  await page.waitForTimeout(100);
  await drain();
  const control = violations.filter((v) => v.where === "positive control");
  expect(control.length, "positive control: a planted inline style must be blocked").toBeGreaterThan(0);
  expect(control[0].directive).toContain("style-src");
});
