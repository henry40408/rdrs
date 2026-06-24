import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { When, Then } = createBdd(test);

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
    const res = await page.request.post(`${serverUrl}/entries/${entryId}/read`);
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
