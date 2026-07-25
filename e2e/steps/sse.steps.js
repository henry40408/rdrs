import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { When, Then } = createBdd(test);

// ── Summary live-update steps ────────────────────────────────────────────────
// Note: "I click the {string} button" is defined in entries.steps.js and reused here.

// The entry row gets a pending/processing badge via SSE after the Summarize POST.
// The mock Kagi server has a brief delay so this transient state is observable.
Then("the entry row shows a pending summary badge", async ({ page }) => {
  await expect
    .poll(
      async () => {
        const badge = page.locator(
          ".summary-badge-pending, .summary-badge-processing"
        ).first();
        return badge.isVisible().catch(() => false);
      },
      { timeout: 3000, intervals: [100, 200] }
    )
    .toBe(true);
});

// Poll until the reading pane's summary container has the completed body text.
// The SSE event triggers a fragment swap of #rp-summary-container — no reload.
Then(
  "without reloading, the reading pane shows the completed summary",
  async ({ page }) => {
    await expect
      .poll(
        async () => {
          const el = page.locator("#rp-summary-container");
          return (await el.textContent()) ?? "";
        },
        { timeout: 5000, intervals: [200, 500] }
      )
      .toContain("E2E mock summary body.");
  }
);

// Poll until the entry row's badge has the "Has Summary" title (completed state).
// The SSE sidebar/entry-row event drives this badge swap without a page reload.
Then("the entry row shows the completed summary badge", async ({ page }) => {
  await expect
    .poll(
      async () => {
        const badge = page.locator('.summary-badge[title="Has Summary"]').first();
        return badge.isVisible().catch(() => false);
      },
      { timeout: 5000, intervals: [200, 500] }
    )
    .toBe(true);
});

// Capture the sidebar unread count before the background read action, then
// POST to mark one entry read. Both happen in one step so the before-snapshot
// is taken atomically before the SSE event can fire.
//
// page.request shares the browser context's session cookie so the POST fires
// as the signed-in user — identical to clicking the read button in the UI —
// and the handler emits an SSE sidebar event the open EventSource receives.
const unreadCountBefore = new WeakMap();

When(
  "a background request marks {string} as read",
  async ({ page, seed, currentUser, serverUrl }, title) => {
    // Snapshot the current unread count BEFORE triggering the mutation.
    const el = page.locator("#unread-count").first();
    const text = (await el.textContent()) || "0";
    unreadCountBefore.set(page, parseInt(text, 10));

    const userId = seed.getUserId(currentUser.username);
    const entryId = seed.findEntryIdByTitle(userId, title);
    // `page.request` shares the session cookie but bypasses the page's patched
    // `fetch`, so it must attach the CSRF token itself — exactly what csrf.js
    // does in the real UI: echo the readable `csrf_token` cookie back as the
    // `X-CSRF-Token` header. Without it the synchronizer-token guard 403s.
    const cookies = await page.context().cookies();
    // The server writes __Host-csrf_token instead of csrf_token whenever the
    // deployment is Secure (see csrf.js's own __Host- handling). E2E
    // currently runs over plain HTTP, so cookie_secure is false and this is
    // not yet load-bearing — but it will break the day E2E moves to HTTPS.
    const csrf =
      cookies.find((c) => c.name === "__Host-csrf_token")?.value ??
      cookies.find((c) => c.name === "csrf_token")?.value ??
      "";
    const res = await page.request.post(`${serverUrl}/entries/${entryId}/read`, {
      headers: { "X-CSRF-Token": csrf },
    });
    // Accept any 2xx (200 OK or 302 redirect after form POST); the important
    // side-effect is the SSE emit, not the response body.
    expect(res.status()).toBeLessThan(400);
  }
);

// Assert the sidebar unread count decreases by 1 within a short window.
// Uses the before-snapshot stored by the When step above. The SSE sidebar event
// triggers rdrs-sidebar.refresh() which calls /api/sidebar and updates the
// badge surgically — no page reload.
Then(
  "within 5 seconds the sidebar unread count decreases by one without a reload",
  async ({ page }) => {
    const before = unreadCountBefore.get(page);
    if (before === undefined) {
      throw new Error('unread count was not captured in the When step');
    }
    const expected = Math.max(0, before - 1);
    const el = page.locator("#unread-count").first();

    await expect
      .poll(
        async () => {
          const text = (await el.textContent()) || "0";
          return parseInt(text, 10);
        },
        { timeout: 5000, intervals: [200, 500] }
      )
      .toBe(expected);
  }
);
