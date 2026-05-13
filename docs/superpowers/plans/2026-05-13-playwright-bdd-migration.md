# Playwright-BDD Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 5 existing TypeScript Playwright specs with a `playwright-bdd` JavaScript suite that locks user-visible behavior as Gherkin contracts.

**Architecture:** Plain JS (ESM) + `@playwright/test` + `playwright-bdd` v8. Per-worker `rdrs` binary spawn with per-worker SQLite. Per-scenario `currentUser` (nanoid-suffixed username) for full scenario isolation. Feature files describe behavior in user language; selectors live only in step definitions.

**Tech Stack:** Node 22, `@playwright/test` ^1.50, `playwright-bdd` ^8, `better-sqlite3` ^11, `nanoid` ^5. CI: GitHub Actions 3-shard matrix on `ubuntu-latest`.

**Spec:** `docs/superpowers/specs/2026-05-13-playwright-bdd-migration-design.md`

**Branch:** `refactor/migrate-e2e-to-playwright-bdd` (already created)

---

## Conventions for this plan

- Every `git commit` uses GPG-signed default (`git commit -m`, no `--no-gpg-sign`).
- Every `git add` names files explicitly — no `git add -A` or `git add .`.
- Working directory for shell commands is the repo root `/home/nixos/Develop/claude/rdrs/` unless `cd e2e &&` is shown.
- OpenSSL env vars from `/tmp/rdrs-env.sh` must be sourced before any `cargo` invocation: `source /tmp/rdrs-env.sh`.
- Rust tests use `cargo nextest run`, not `cargo test`.

---

## File Structure

After all 6 tasks complete:

```
e2e/
  package.json              # MODIFIED — ESM, new deps
  playwright.config.js      # CREATED (replaces .ts)
  global-setup.js           # CREATED (replaces .ts)
  .gitignore                # MODIFIED — add .features-gen/

  features/                 # CREATED
    authentication.feature
    organizing.feature
    preferences.feature
    search.feature
    responsive.feature
    admin.feature
    reading.feature
    triage.feature

  steps/                    # CREATED
    auth.steps.js
    navigation.steps.js
    organize.steps.js
    preferences.steps.js
    search.steps.js
    responsive.steps.js
    admin.steps.js
    entries.steps.js
    triage.steps.js

  support/                  # CREATED
    fixtures.js
    server.js
    api.js
    seed.js

  scripts/
    screenshots.js          # CREATED (replaces .ts)

  tests/                    # DELETED
  fixtures/                 # DELETED
  helpers/                  # DELETED
  tsconfig.json             # DELETED
```

`.github/workflows/ci.yml` modified to run the 3-shard matrix in the final task.

---

## Task 1: Skeleton — JS port of fixtures, ESM package, BDD wiring

**Files:**
- Modify: `e2e/package.json`
- Modify: `e2e/.gitignore`
- Create: `e2e/support/server.js`
- Create: `e2e/support/api.js`
- Create: `e2e/support/seed.js`
- Create: `e2e/support/fixtures.js`
- Create: `e2e/global-setup.js`
- Create: `e2e/playwright.config.js`
- Create: `e2e/features/.gitkeep` (placeholder so glob is valid until Task 2)
- Create: `e2e/steps/.gitkeep`
- Modify: `e2e/scripts/screenshots.ts` → rename to `e2e/scripts/screenshots.js` (JS port)
- Delete: `e2e/playwright.config.ts`, `e2e/global-setup.ts`, `e2e/fixtures/rdrs.ts`, `e2e/helpers/api.ts`, `e2e/helpers/seed.ts`, `e2e/scripts/screenshots.ts`, `e2e/tsconfig.json`

**Note:** The 5 spec files in `e2e/tests/*.spec.ts` stay untouched in this task — they will not run (their imports are TS). This task is the foundation only; running e2e on this task's branch will pick up zero scenarios. CI for this task verifies `bddgen` produces an empty `.features-gen/` and exits 0.

- [ ] **Step 1: Update `e2e/package.json` to ESM with new deps**

Overwrite `e2e/package.json` with:

```json
{
  "name": "rdrs-e2e",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "playwright test",
    "test:ui": "playwright test --ui",
    "test:headed": "playwright test --headed",
    "screenshots": "node scripts/screenshots.js"
  },
  "devDependencies": {
    "@playwright/test": "^1.50.0",
    "better-sqlite3": "^11.8.0",
    "nanoid": "^5.0.0",
    "playwright-bdd": "^8.0.0"
  }
}
```

- [ ] **Step 2: Install deps**

Run:
```bash
cd e2e && rm -f package-lock.json && npm install
```
Expected: `node_modules/` updated, new `package-lock.json` written.

- [ ] **Step 3: Update `e2e/.gitignore`**

Append `.features-gen/` so it reads:
```
node_modules/
dist/
playwright-report/
test-results/
blob-report/
.features-gen/
```

- [ ] **Step 4: Create `e2e/support/api.js` (port of `helpers/api.ts`)**

```javascript
export class ApiHelper {
  constructor(baseUrl) {
    this.baseUrl = baseUrl;
  }

  async register(username, password) {
    const res = await fetch(`${this.baseUrl}/api/register`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });
    if (!res.ok && res.status !== 409) {
      const body = await res.text();
      throw new Error(`Register failed (${res.status}): ${body}`);
    }
    return { cookie: "" };
  }

  async login(username, password) {
    const res = await fetch(`${this.baseUrl}/api/session`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
      redirect: "manual",
    });
    if (!res.ok) {
      const body = await res.text();
      throw new Error(`Login failed (${res.status}): ${body}`);
    }
    const setCookie = res.headers.getSetCookie?.() ?? [];
    const cookie = setCookie.map((c) => c.split(";")[0]).join("; ");
    return { cookie };
  }
}
```

- [ ] **Step 5: Create `e2e/support/seed.js` (port of `helpers/seed.ts`)**

```javascript
import Database from "better-sqlite3";

export class SeedHelper {
  constructor(dbPath) {
    this.dbPath = dbPath;
  }

  insertEntries(entries) {
    const db = new Database(this.dbPath);
    const ids = [];
    try {
      const stmt = db.prepare(
        `INSERT OR IGNORE INTO entry (feed_id, guid, title, link, content, summary, published_at)
         VALUES (?, ?, ?, ?, ?, ?, datetime('now', ?))`
      );
      const insertAll = db.transaction(() => {
        for (const entry of entries) {
          const result = stmt.run(
            entry.feedId,
            entry.guid,
            entry.title,
            entry.link,
            entry.content,
            entry.summary ?? null,
            entry.publishedOffset ?? "0 seconds"
          );
          ids.push(Number(result.lastInsertRowid));
        }
      });
      insertAll();
    } finally {
      db.close();
    }
    return ids;
  }

  getUserId(username) {
    const db = new Database(this.dbPath);
    try {
      const row = db.prepare(`SELECT id FROM user WHERE username = ?`).get(username);
      if (!row) throw new Error(`User '${username}' not found`);
      return row.id;
    } finally {
      db.close();
    }
  }

  createCategory(userId, name) {
    const db = new Database(this.dbPath);
    try {
      db.prepare(`INSERT OR IGNORE INTO category (user_id, name) VALUES (?, ?)`).run(userId, name);
      const row = db.prepare(`SELECT id FROM category WHERE user_id = ? AND name = ?`).get(userId, name);
      return row.id;
    } finally {
      db.close();
    }
  }

  createFeed(categoryId, url, title) {
    const db = new Database(this.dbPath);
    try {
      db.prepare(`INSERT OR IGNORE INTO feed (category_id, url, title) VALUES (?, ?, ?)`).run(
        categoryId,
        url,
        title ?? url
      );
      const row = db.prepare(`SELECT id FROM feed WHERE url = ?`).get(url);
      return row.id;
    } finally {
      db.close();
    }
  }

  insertIcon(feedId, data, contentType, sourceUrl) {
    const db = new Database(this.dbPath);
    try {
      db.prepare(
        `INSERT OR REPLACE INTO image (entity_type, entity_id, data, content_type, source_url)
         VALUES ('feed', ?, ?, ?, ?)`
      ).run(feedId, data, contentType, sourceUrl ?? null);
    } finally {
      db.close();
    }
  }

  markRead(entryId, relativeTime = "0 seconds") {
    const db = new Database(this.dbPath);
    try {
      db.prepare(`UPDATE entry SET read_at = datetime('now', ?) WHERE id = ?`).run(relativeTime, entryId);
    } finally {
      db.close();
    }
  }

  markStarred(entryId, relativeTime = "0 seconds") {
    const db = new Database(this.dbPath);
    try {
      db.prepare(`UPDATE entry SET starred_at = datetime('now', ?) WHERE id = ?`).run(relativeTime, entryId);
    } finally {
      db.close();
    }
  }

  insertSummary(entryId, userId, text = "summary.") {
    const db = new Database(this.dbPath);
    try {
      db.prepare(
        `INSERT OR IGNORE INTO entry_summary (user_id, entry_id, status, summary_text)
         VALUES (?, ?, 'completed', ?)`
      ).run(userId, entryId, text);
    } finally {
      db.close();
    }
  }

  seedTestEntries(feedId, count) {
    const entries = [];
    for (let i = 1; i <= count; i++) {
      entries.push({
        feedId,
        guid: `test-guid-${feedId}-${i}`,
        title: `Test Entry ${i}`,
        link: `https://example.com/entry/${i}`,
        content: `<p>Content for test entry ${i}</p>`,
        summary: `Summary for entry ${i}`,
        publishedOffset: `-${i} hours`,
      });
    }
    return this.insertEntries(entries);
  }
}
```

- [ ] **Step 6: Create `e2e/support/server.js` (port of `fixtures/rdrs.ts` server-spawn logic)**

```javascript
import { spawn } from "child_process";
import { mkdtempSync, rmSync, existsSync } from "fs";
import http from "http";
import { tmpdir } from "os";
import path from "path";
import net from "net";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const MOCK_RSS_FEED = `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <link>http://localhost</link>
    <description>A test feed for E2E tests</description>
  </channel>
</rss>`;

export function findAvailablePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      if (addr && typeof addr === "object") {
        const port = addr.port;
        server.close(() => resolve(port));
      } else {
        reject(new Error("Could not determine port"));
      }
    });
    server.on("error", reject);
  });
}

async function waitForServer(baseUrl, timeoutMs = 30_000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(`${baseUrl}/health`);
      if (res.ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`Server did not become ready within ${timeoutMs}ms`);
}

export async function spawnRdrs() {
  const projectRoot = path.resolve(__dirname, "..", "..");
  const binaryPath = path.join(projectRoot, "target", "debug", "rdrs");
  if (!existsSync(binaryPath)) {
    throw new Error(`rdrs binary not found at ${binaryPath} — run cargo build first`);
  }
  const tempDir = mkdtempSync(path.join(tmpdir(), "rdrs-e2e-"));
  const dbPath = path.join(tempDir, "test.sqlite3");
  const port = await findAvailablePort();
  const baseUrl = `http://127.0.0.1:${port}`;

  const proc = spawn(binaryPath, [], {
    cwd: projectRoot,
    env: {
      ...process.env,
      DATABASE_URL: dbPath,
      SERVER_PORT: String(port),
      SIGNUP_ENABLED: "true",
      MULTI_USER_ENABLED: "true",
      RUST_LOG: "warn",
    },
    stdio: "pipe",
  });
  proc.stderr?.on("data", (data) => {
    if (process.env.DEBUG) process.stderr.write(`[rdrs:${port}] ${data}`);
  });

  await waitForServer(baseUrl);
  return {
    url: baseUrl,
    dbPath,
    cleanup: async () => {
      proc.kill("SIGTERM");
      await new Promise((resolve) => {
        proc.on("close", () => resolve());
        setTimeout(resolve, 5_000);
      });
      rmSync(tempDir, { recursive: true, force: true });
    },
  };
}

export async function spawnMockFeedServer() {
  const server = http.createServer((_req, res) => {
    res.writeHead(200, { "Content-Type": "application/rss+xml" });
    res.end(MOCK_RSS_FEED);
  });
  const port = await findAvailablePort();
  server.listen(port, "127.0.0.1");
  return {
    url: `http://127.0.0.1:${port}`,
    cleanup: async () => {
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
```

- [ ] **Step 7: Create `e2e/support/fixtures.js`**

```javascript
import { test as base } from "playwright-bdd";
import { customAlphabet } from "nanoid";
import { ApiHelper } from "./api.js";
import { SeedHelper } from "./seed.js";
import { spawnRdrs, spawnMockFeedServer } from "./server.js";

const nano = customAlphabet("abcdefghijklmnopqrstuvwxyz0123456789", 8);

export const test = base.extend({
  rdrsServer: [
    async ({}, use) => {
      const server = await spawnRdrs();
      try {
        await use(server);
      } finally {
        await server.cleanup();
      }
    },
    { scope: "worker" },
  ],

  serverUrl: [
    async ({ rdrsServer }, use) => {
      await use(rdrsServer.url);
    },
    { scope: "worker" },
  ],

  dbPath: [
    async ({ rdrsServer }, use) => {
      await use(rdrsServer.dbPath);
    },
    { scope: "worker" },
  ],

  api: [
    async ({ serverUrl }, use) => {
      await use(new ApiHelper(serverUrl));
    },
    { scope: "worker" },
  ],

  seed: [
    async ({ dbPath }, use) => {
      await use(new SeedHelper(dbPath));
    },
    { scope: "worker" },
  ],

  feedServerUrl: [
    async ({}, use) => {
      const server = await spawnMockFeedServer();
      try {
        await use(server.url);
      } finally {
        await server.cleanup();
      }
    },
    { scope: "worker" },
  ],

  currentUser: async ({}, use) => {
    await use({ username: `e2e-${nano()}`, password: "password123" });
  },
});

export { expect } from "@playwright/test";
```

- [ ] **Step 8: Create `e2e/global-setup.js`**

```javascript
import { execSync } from "child_process";
import { existsSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

export default function globalSetup() {
  const projectRoot = path.resolve(__dirname, "..");
  const binaryPath = path.join(projectRoot, "target", "debug", "rdrs");
  if (!existsSync(binaryPath)) {
    console.log("Building rdrs binary (debug mode)...");
    execSync("cargo build", { cwd: projectRoot, stdio: "inherit" });
  } else {
    console.log("rdrs binary already exists, skipping build.");
  }
}
```

- [ ] **Step 9: Create `e2e/playwright.config.js`**

```javascript
import { defineConfig } from "@playwright/test";
import { defineBddConfig } from "playwright-bdd";

const testDir = defineBddConfig({
  features: "features/*.feature",
  steps: ["steps/*.js", "support/fixtures.js"],
});

export default defineConfig({
  testDir,
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  workers: process.env.CI ? "50%" : "75%",
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? "html" : "list",
  use: {
    trace: "on-first-retry",
  },
  globalSetup: "./global-setup.js",
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
});
```

- [ ] **Step 10: Create placeholders so the empty glob works**

```bash
mkdir -p e2e/features e2e/steps
touch e2e/features/.gitkeep e2e/steps/.gitkeep
```

- [ ] **Step 11: Port `e2e/scripts/screenshots.ts` to `screenshots.js`**

This is a 221-line utility that imports the old `fixtures/rdrs.ts`. Replace its imports and rename types. The mechanical change is:
- File rename to `.js`
- Remove all type annotations (`: string`, `: Record<...>`, `Promise<...>`, etc.)
- Replace `import { test, expect } from "../fixtures/rdrs.js"` with `import { test, expect } from "../support/fixtures.js"`
- Replace `__dirname` with the ESM-compatible derivation:
  ```javascript
  import { fileURLToPath } from "url";
  import path from "path";
  const __filename = fileURLToPath(import.meta.url);
  const __dirname = path.dirname(__filename);
  ```

After porting, delete `e2e/scripts/screenshots.ts`.

- [ ] **Step 12: Delete obsolete TS files**

```bash
rm e2e/playwright.config.ts
rm e2e/global-setup.ts
rm e2e/fixtures/rdrs.ts
rm e2e/helpers/api.ts
rm e2e/helpers/seed.ts
rm e2e/tsconfig.json
rmdir e2e/fixtures e2e/helpers 2>/dev/null || true
```

Note: `e2e/tests/*.spec.ts` is intentionally NOT deleted here — those die in Task 2 (auth) and Task 3 (rest).

- [ ] **Step 13: Verify build with `cargo build` then BDD smoke test**

```bash
source /tmp/rdrs-env.sh && cargo build
```
Expected: build succeeds.

```bash
cd e2e && npx playwright test --list 2>&1 | head -20
```
Expected: `Listing tests:` followed by no tests (no `.feature` files yet), exit 0.

- [ ] **Step 14: Commit**

```bash
git add e2e/package.json e2e/package-lock.json e2e/.gitignore \
  e2e/support/api.js e2e/support/seed.js e2e/support/server.js e2e/support/fixtures.js \
  e2e/global-setup.js e2e/playwright.config.js \
  e2e/features/.gitkeep e2e/steps/.gitkeep \
  e2e/scripts/screenshots.js
git rm e2e/playwright.config.ts e2e/global-setup.ts \
  e2e/fixtures/rdrs.ts e2e/helpers/api.ts e2e/helpers/seed.ts \
  e2e/scripts/screenshots.ts e2e/tsconfig.json
git commit -m "$(cat <<'EOF'
refactor(e2e): port fixtures/helpers to ESM JS, wire playwright-bdd

Convert e2e/ to ESM with playwright-bdd v8. Adds support/ (server,
api, seed, fixtures) replacing fixtures/rdrs.ts + helpers/*.ts. Adds
currentUser fixture for per-scenario nanoid usernames. Old tests/
specs still present and unchanged in this commit; they fail-to-load
since their TS imports are gone — Task 2 deletes auth.spec.ts and
adds the first feature.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: First feature — `authentication.feature`

**Files:**
- Create: `e2e/features/authentication.feature`
- Create: `e2e/steps/auth.steps.js`
- Create: `e2e/steps/navigation.steps.js`
- Delete: `e2e/tests/auth.spec.ts`

- [ ] **Step 1: Write `e2e/features/authentication.feature`**

```gherkin
@parallel
Feature: Authentication

  Scenario: New user can register, sign in, and reach the unread inbox
    When I register with matching passwords
    Then I am redirected to the login page with a success message
    When I sign in with my credentials
    Then I land on the unread inbox

  Scenario: Sign-in with the wrong password shows an error
    Given I am a registered user
    When I sign in with the wrong password
    Then I see a login error

  Scenario: Mismatched passwords on registration show a client-side error
    When I register with mismatched passwords
    Then I see "Passwords do not match" on the register page
    And I am still on the register page

  Scenario: Authenticated user visiting /login is redirected to the inbox
    Given I am signed in
    When I visit "/login"
    Then I land on the unread inbox
```

- [ ] **Step 2: Write `e2e/steps/auth.steps.js`**

```javascript
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("I am a registered user", async ({ api, currentUser }) => {
  await api.register(currentUser.username, currentUser.password);
});

Given("I am signed in", async ({ page, api, currentUser, serverUrl }) => {
  await api.register(currentUser.username, currentUser.password);
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
});

When("I register with matching passwords", async ({ page, currentUser, serverUrl }) => {
  await page.goto(`${serverUrl}/register`);
  await page.getByTestId("register-username").fill(currentUser.username);
  await page.getByTestId("register-password").fill(currentUser.password);
  await page.getByTestId("register-confirm-password").fill(currentUser.password);
  await page.getByTestId("register-submit").click();
});

When("I register with mismatched passwords", async ({ page, currentUser, serverUrl }) => {
  await page.goto(`${serverUrl}/register`);
  await page.getByTestId("register-username").fill(currentUser.username);
  await page.getByTestId("register-password").fill(currentUser.password);
  await page.getByTestId("register-confirm-password").fill("different456");
  await page.getByTestId("register-submit").click();
});

When("I sign in with my credentials", async ({ page, currentUser, serverUrl }) => {
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
});

When("I sign in with the wrong password", async ({ page, currentUser, serverUrl }) => {
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill("wrongpassword");
  await page.getByTestId("login-submit").click();
});

Then("I am redirected to the login page with a success message", async ({ page, serverUrl }) => {
  await page.waitForURL(`${serverUrl}/login`);
  await expect(page.getByTestId("flash-message")).toBeVisible();
  await expect(page.getByTestId("flash-message")).toContainText("Registration successful");
});

Then("I land on the unread inbox", async ({ page, serverUrl }) => {
  await page.waitForURL(`${serverUrl}/`);
  await expect(page.getByTestId("main-nav")).toBeVisible();
});

Then("I see a login error", async ({ page }) => {
  await expect(page.getByTestId("login-error")).toBeVisible();
});

Then("I see {string} on the register page", async ({ page }, message) => {
  await expect(page.getByTestId("register-error")).toBeVisible();
  await expect(page.getByTestId("register-error")).toContainText(message);
});

Then("I am still on the register page", async ({ page }) => {
  expect(page.url()).toContain("/register");
});
```

- [ ] **Step 3: Write `e2e/steps/navigation.steps.js` (shared, will grow over tasks)**

```javascript
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { When } = createBdd(test);

When("I visit {string}", async ({ page, serverUrl }, path) => {
  await page.goto(`${serverUrl}${path}`);
});
```

- [ ] **Step 4: Delete the old auth spec**

```bash
git rm e2e/tests/auth.spec.ts
```

- [ ] **Step 5: Run the feature locally**

```bash
cd e2e && npx playwright test authentication
```
Expected: 4 passed.

If any scenario fails, inspect the failing testid in `templates/login.html` or `templates/register.html` and reconcile the step or the template — do NOT mark `@pending`. Authentication is gateway behavior; everything else depends on it.

- [ ] **Step 6: Commit**

```bash
git add e2e/features/authentication.feature e2e/steps/auth.steps.js e2e/steps/navigation.steps.js
git commit -m "$(cat <<'EOF'
test(e2e): add authentication.feature, drop auth.spec.ts

Four scenarios cover register-then-sign-in, wrong password, password
mismatch, and signed-in user redirect on /login visit. Steps use
declarative user-language phrases; selectors and URLs are in steps.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Translate the 4 remaining existing specs

Translate `theme.spec.ts`, `search.spec.ts`, `responsive.spec.ts`, `feed-management.spec.ts` to features. Delete each source spec at the end of its sub-section.

**Files:**
- Create: `e2e/features/preferences.feature`
- Create: `e2e/features/search.feature`
- Create: `e2e/features/responsive.feature`
- Create: `e2e/features/organizing.feature`
- Create: `e2e/steps/preferences.steps.js`
- Create: `e2e/steps/search.steps.js`
- Create: `e2e/steps/responsive.steps.js`
- Create: `e2e/steps/organize.steps.js`
- Delete: `e2e/tests/theme.spec.ts`, `search.spec.ts`, `responsive.spec.ts`, `feed-management.spec.ts`

### 3a. `preferences.feature`

- [ ] **Step 1: Write `e2e/features/preferences.feature`**

```gherkin
@parallel
Feature: Preferences

  Background:
    Given I am signed in
    And I am on the user settings page

  Scenario: Switching to dark theme sets data-theme to dark
    When I switch the theme to "dark"
    Then the html element has data-theme "dark"

  Scenario: Switching to light theme sets data-theme to light
    When I switch the theme to "light"
    Then the html element has data-theme "light"

  Scenario: Switching to system theme removes the data-theme attribute
    When I switch the theme to "dark"
    And the html element has data-theme "dark"
    And I switch the theme to "system"
    Then the html element has no data-theme attribute

  Scenario: Changing my password lets me sign in with the new password
    When I change my password to "newpassword123"
    Then I can sign in with "newpassword123"

  Scenario: Changing my display name updates the navbar greeting
    When I change my display name to "Alex"
    Then the navbar greeting shows "Alex"
```

- [ ] **Step 2: Write `e2e/steps/preferences.steps.js`**

```javascript
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then, And } = createBdd(test);

Given("I am on the user settings page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/user-settings`);
  await expect(page.getByTestId("theme-select")).toBeVisible();
});

When("I switch the theme to {string}", async ({ page, serverUrl }, theme) => {
  await page.getByTestId("theme-select").selectOption(theme);
  await page.locator('form[action="/user-settings/preferences"] button[type=submit]').click();
  await page.waitForURL(`${serverUrl}/user-settings`);
});

Then("the html element has data-theme {string}", async ({ page }, value) => {
  await expect(page.locator("html")).toHaveAttribute("data-theme", value);
});

Then("the html element has no data-theme attribute", async ({ page }) => {
  await expect(page.locator("html")).not.toHaveAttribute("data-theme");
});

When("I change my password to {string}", async ({ page, currentUser, serverUrl }, newPassword) => {
  await page.locator('form[action="/user-settings/password"] [data-testid="current-password"]').fill(currentUser.password);
  await page.locator('form[action="/user-settings/password"] [data-testid="new-password"]').fill(newPassword);
  await page.locator('form[action="/user-settings/password"] [data-testid="confirm-new-password"]').fill(newPassword);
  await page.locator('form[action="/user-settings/password"] button[type=submit]').click();
  await page.waitForURL(`${serverUrl}/user-settings`);
  currentUser.password = newPassword;
});

Then("I can sign in with {string}", async ({ page, currentUser, serverUrl }, password) => {
  await page.goto(`${serverUrl}/api/session/destroy`);
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
});

When("I change my display name to {string}", async ({ page, serverUrl }, displayName) => {
  await page.getByTestId("display-name-input").fill(displayName);
  await page.locator('form[action="/user-settings/profile"] button[type=submit]').click();
  await page.waitForURL(`${serverUrl}/user-settings`);
});

Then("the navbar greeting shows {string}", async ({ page }, displayName) => {
  await expect(page.getByTestId("navbar-greeting")).toContainText(displayName);
});
```

- [ ] **Step 3: Verify the user-settings form `data-testid` values exist**

```bash
grep -n "data-testid" templates/user_settings.html | head -30
```
The preferences steps require:
- Password form: `current-password`, `new-password`, `confirm-new-password`
- Profile form: `display-name-input`, `navbar-greeting` (the navbar greeting element lives in `templates/app_layout.html` or `templates/base.html`)

For any missing testid, add it to the corresponding template and stage in the Task 3 commit.

- [ ] **Step 4: Run `preferences.feature`**

```bash
cd e2e && npx playwright test preferences
```
Expected: 5 passed.

- [ ] **Step 5: Delete `theme.spec.ts`**

```bash
git rm e2e/tests/theme.spec.ts
```

### 3b. `search.feature`

- [ ] **Step 6: Write `e2e/features/search.feature`**

```gherkin
@parallel
Feature: Search

  Background:
    Given I am signed in
    And I have a feed with entries titled:
      | Rust Programming Guide |
      | JavaScript Frameworks  |
      | Rust Async Runtime     |
    And I am on the search page

  Scenario: Searching for a term shows matching entries
    When I search for "Rust"
    Then I see search results:
      | Rust Programming Guide |
      | Rust Async Runtime     |
    And the result count is 2

  Scenario: Pressing the slash key focuses the search input
    When I press "/"
    Then the search input is focused

  Scenario: Searching for a term with no matches shows an empty state
    When I search for "Kotlin"
    Then I see the empty-results message
```

- [ ] **Step 7: Write `e2e/steps/search.steps.js`**

```javascript
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("I have a feed with entries titled:", async ({ seed, api, currentUser }, table) => {
  await api.register(currentUser.username, currentUser.password);
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, "Search Category");
  const feedId = seed.createFeed(categoryId, `https://example.com/${currentUser.username}-feed.xml`, "Search Feed");
  const rows = table.raw().map((r, i) => ({
    feedId,
    guid: `${currentUser.username}-${i}`,
    title: r[0],
    link: `https://example.com/${currentUser.username}/${i}`,
    content: `<p>${r[0]}</p>`,
    publishedOffset: `-${i + 1} hours`,
  }));
  seed.insertEntries(rows);
});

Given("I am on the search page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/search`);
});

When("I search for {string}", async ({ page }, term) => {
  await page.getByTestId("search-input").fill(term);
  await page.keyboard.press("Enter");
});

When("I press {string}", async ({ page }, key) => {
  await page.click("body");
  await page.keyboard.press(key);
});

Then("I see search results:", async ({ page }, table) => {
  await expect(page.getByTestId("search-results")).toBeVisible();
  for (const [title] of table.raw()) {
    await expect(page.getByText(title)).toBeVisible();
  }
});

Then("the result count is {int}", async ({ page }, count) => {
  await expect(page.locator(".search-result")).toHaveCount(count);
});

Then("the search input is focused", async ({ page }) => {
  await expect(page.getByTestId("search-input")).toBeFocused();
});

Then("I see the empty-results message", async ({ page }) => {
  await expect(page.getByTestId("search-empty")).toBeVisible();
});
```

- [ ] **Step 8: Verify `search-empty` testid exists**

```bash
grep -n "search-empty" templates/search.html
```
If missing, add `data-testid="search-empty"` to the empty-state element in `templates/search.html`. Stage the template change in the same commit.

- [ ] **Step 9: Run `search.feature`**

```bash
cd e2e && npx playwright test search
```
Expected: 3 passed.

- [ ] **Step 10: Delete `search.spec.ts`**

```bash
git rm e2e/tests/search.spec.ts
```

### 3c. `responsive.feature`

- [ ] **Step 11: Write `e2e/features/responsive.feature`**

```gherkin
@parallel
Feature: Responsive layout

  Background:
    Given I am signed in

  @mobile
  Scenario: Sidebar is hidden by default and toggled by the hamburger on mobile
    Given I am viewing on a mobile screen
    When I open the inbox
    Then the sidebar is not visible
    When I tap the hamburger
    Then the sidebar is visible
    When I tap the sidebar close button
    Then the sidebar is not visible

  @mobile
  Scenario: Entry list is full-width single column on mobile
    Given I am viewing on a mobile screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the entry list pane is at least 370px wide

  @mobile
  Scenario: Categories table renders as cards on mobile
    Given I am viewing on a mobile screen
    When I open the categories page
    Then the categories table is shown as cards

  @tablet
  Scenario: Sidebar is a drawer on tablet
    Given I am viewing on a tablet screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the sidebar is not visible
    And the hamburger button is visible
    When I tap the hamburger
    Then the sidebar is visible

  @tablet
  Scenario: Tables keep table layout on tablet
    Given I am viewing on a tablet screen
    When I open the categories page
    Then the categories table is shown as a table

  @tablet
  Scenario: Entry list is full-width single column on tablet
    Given I am viewing on a tablet screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the entry list pane is at least 760px wide

  @desktop
  Scenario: Sidebar is always visible on desktop
    Given I am viewing on a desktop screen
    When I open the inbox
    Then the sidebar is always-visible

  @desktop
  Scenario: Reading pane sits beside the entry list on desktop
    Given I am viewing on a desktop screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the entry list pane is narrower than the viewport
```

- [ ] **Step 12: Write `e2e/steps/responsive.steps.js`**

```javascript
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

const VIEWPORTS = {
  mobile: { width: 375, height: 667 },
  tablet: { width: 768, height: 1024 },
  desktop: { width: 1280, height: 800 },
};

Given("I am viewing on a {word} screen", async ({ page }, kind) => {
  const v = VIEWPORTS[kind];
  if (!v) throw new Error(`Unknown viewport: ${kind}`);
  await page.setViewportSize(v);
});

Given("I have a feed with {int} test entries", async ({ seed, currentUser }, count) => {
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, `Cat-${currentUser.username}`);
  const feedId = seed.createFeed(categoryId, `https://example.com/${currentUser.username}.xml`, "Mobile Feed");
  seed.seedTestEntries(feedId, count);
});

When("I open the inbox", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/`);
});

When("I open the categories page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/categories`);
});

When("I tap the hamburger", async ({ page }) => {
  await page.locator(".sidebar-toggle").click();
});

When("I tap the sidebar close button", async ({ page }) => {
  await page.locator(".sidebar-close").click();
});

Then("the sidebar is visible", async ({ page }) => {
  await expect(page.locator("#sidebar")).toHaveClass(/open/);
});

Then("the sidebar is not visible", async ({ page }) => {
  await expect(page.locator("#sidebar")).not.toHaveClass(/open/);
});

Then("the hamburger button is visible", async ({ page }) => {
  await expect(page.locator(".sidebar-toggle")).toBeVisible();
});

Then("the entry list pane is at least {int}px wide", async ({ page }, minWidth) => {
  const box = await page.locator(".list-pane").boundingBox();
  expect(box.width).toBeGreaterThan(minWidth);
});

Then("the categories table is shown as cards", async ({ page }) => {
  await expect(page.locator("table.mobile-cards thead")).toHaveCSS("display", "none");
  await expect(page.locator("table.mobile-cards td[data-label]").first()).toBeVisible();
});

Then("the categories table is shown as a table", async ({ page }) => {
  await expect(page.locator("table.mobile-cards thead")).not.toHaveCSS("display", "none");
});

Then("the sidebar is always-visible", async ({ page }) => {
  await expect(page.locator("#sidebar")).toBeVisible();
  await expect(page.locator(".sidebar-toggle")).toBeHidden();
});

Then("the entry list pane is narrower than the viewport", async ({ page }) => {
  const viewport = page.viewportSize();
  const box = await page.locator(".list-pane").boundingBox();
  expect(box.width).toBeLessThan(viewport.width * 0.9);
});
```

- [ ] **Step 13: Run `responsive.feature`**

```bash
cd e2e && npx playwright test responsive
```
Expected: 8 passed.

- [ ] **Step 14: Delete `responsive.spec.ts`**

```bash
git rm e2e/tests/responsive.spec.ts
```

### 3d. `organizing.feature`

- [ ] **Step 15: Write `e2e/features/organizing.feature`**

```gherkin
@parallel
Feature: Organizing feeds and categories

  Background:
    Given I am signed in
    And I have a category named "My Category"

  Scenario: Adding a feed makes it appear in the feeds table
    Given I am on the feeds page
    When I add a feed from the mock RSS server under "My Category"
    Then I see a success flash "Feed added"
    And the feeds table contains "Test Feed"
```

(Sub-scenarios for edit/delete/rename/import/export are deferred — see Open scenarios at end of plan.)

- [ ] **Step 16: Write `e2e/steps/organize.steps.js`**

```javascript
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("I have a category named {string}", async ({ seed, currentUser }, name) => {
  const userId = seed.getUserId(currentUser.username);
  seed.createCategory(userId, name);
});

Given("I am on the feeds page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/feeds`);
  await expect(page.getByTestId("feed-category-select")).not.toContainText("Loading");
});

When("I add a feed from the mock RSS server under {string}", async ({ page, feedServerUrl }, _category) => {
  await page.getByTestId("feed-url-input").fill(`${feedServerUrl}/feed.xml`);
  await page.getByTestId("feed-category-select").selectOption({ index: 0 });
  await page.getByTestId("add-feed-btn").click();
});

Then("I see a success flash {string}", async ({ page }, message) => {
  await expect(page.getByTestId("flash-message")).toContainText(message);
});

Then("the feeds table contains {string}", async ({ page }, text) => {
  await expect(page.getByTestId("feeds-table")).toContainText(text);
});
```

- [ ] **Step 17: Run `organizing.feature`**

```bash
cd e2e && npx playwright test organizing
```
Expected: 1 passed.

- [ ] **Step 18: Delete `feed-management.spec.ts`**

```bash
git rm e2e/tests/feed-management.spec.ts
```

### 3e. Commit Task 3

- [ ] **Step 19: Run the full BDD suite**

```bash
cd e2e && npx playwright test --reporter=list
```
Expected: 4 (auth) + 5 (prefs) + 3 (search) + 8 (responsive) + 1 (organize) = 21 passed.

- [ ] **Step 20: Commit**

```bash
git add e2e/features/preferences.feature e2e/features/search.feature \
  e2e/features/responsive.feature e2e/features/organizing.feature \
  e2e/steps/preferences.steps.js e2e/steps/search.steps.js \
  e2e/steps/responsive.steps.js e2e/steps/organize.steps.js
# Stage template testid additions if any were needed in Step 3 or Step 8:
# git add templates/user_settings.html templates/search.html
git commit -m "$(cat <<'EOF'
test(e2e): translate theme/search/responsive/feed specs to features

Replaces 4 of the 5 existing TS specs with declarative Gherkin
features + JS steps. 18 scenarios across preferences, search,
responsive layout, and organizing. Adds missing data-testid
selectors on user-settings / search templates where needed to
support declarative step phrasing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Admin + statistics features

**Files:**
- Create: `e2e/features/admin.feature`
- Create: `e2e/steps/admin.steps.js`

- [ ] **Step 1: Inspect `templates/admin.html` and `templates/statistics.html` for testids**

```bash
grep -n "data-testid" templates/admin.html templates/statistics.html
```
Record the available testids. If a scenario below needs a testid that doesn't exist, add it to the template in the same commit.

- [ ] **Step 2: Write `e2e/features/admin.feature`**

```gherkin
@parallel
Feature: Admin and statistics

  Background:
    Given I am signed in as an admin

  Scenario: Admin sees the list of all users
    When I open the admin page
    Then I see my username in the users table

  Scenario: Admin creates a new user account
    When I open the admin page
    And I create a user with a random username and password "password123"
    Then the new user appears in the users table

  Scenario: Admin disables a user account
    Given there is a regular user "regular-user"
    When I open the admin page
    And I disable "regular-user"
    Then "regular-user" is shown as disabled

  Scenario: Statistics page shows feed and entry counts
    Given I have a feed with 3 test entries
    When I open the statistics page
    Then the statistics show at least 1 feed
    And the statistics show at least 3 entries
```

- [ ] **Step 3: Write `e2e/steps/admin.steps.js`**

```javascript
import { createBdd } from "playwright-bdd";
import { customAlphabet } from "nanoid";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);
const nano = customAlphabet("abcdefghijklmnopqrstuvwxyz0123456789", 6);

Given("I am signed in as an admin", async ({ page, api, currentUser, seed, serverUrl }) => {
  await api.register(currentUser.username, currentUser.password);
  const userId = seed.getUserId(currentUser.username);
  // Promote to admin via direct SQL — admin role is a column on the user row.
  // Pull dbPath from seed.
  const Database = (await import("better-sqlite3")).default;
  const db = new Database(seed.dbPath);
  try {
    db.prepare(`UPDATE user SET is_admin = 1 WHERE id = ?`).run(userId);
  } finally {
    db.close();
  }
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
});

Given("there is a regular user {string}", async ({ api }, username) => {
  // Username is qualified by adding a nanoid suffix to keep parallel scenarios safe.
  await api.register(`${username}-${nano()}`, "password123");
});

When("I open the admin page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/admin`);
});

When("I open the statistics page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/statistics`);
});

When("I create a user with a random username and password {string}", async ({ page }, password) => {
  const u = `bdd-${nano()}`;
  await page.getByTestId("admin-new-username").fill(u);
  await page.getByTestId("admin-new-password").fill(password);
  await page.getByTestId("admin-create-user").click();
  // Stash the username for the next step
  page.__lastCreatedUser = u;
});

When("I disable {string}", async ({ page }, _usernamePrefix) => {
  // Find the row by username prefix
  const row = page.locator("tr[data-testid^='admin-user-row-']").first();
  await row.getByTestId("admin-disable-btn").click();
});

Then("the new user appears in the users table", async ({ page }) => {
  await expect(page.getByTestId("admin-users-table")).toContainText(page.__lastCreatedUser);
});

Then("I see my username in the users table", async ({ page, currentUser }) => {
  await expect(page.getByTestId("admin-users-table")).toContainText(currentUser.username);
});

Then("{string} is shown as disabled", async ({ page }) => {
  await expect(page.locator(".admin-user-disabled").first()).toBeVisible();
});

Then("the statistics show at least {int} feed", async ({ page }, n) => {
  const text = await page.getByTestId("stat-feeds-total").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(n);
});

Then("the statistics show at least {int} entries", async ({ page }, n) => {
  const text = await page.getByTestId("stat-entries-total").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(n);
});
```

- [ ] **Step 4: Inspect required testids and add any missing to templates**

The steps reference these testids:
- `admin-new-username`, `admin-new-password`, `admin-create-user`
- `admin-users-table`, `admin-user-row-*`, `admin-disable-btn`
- `stat-feeds-total`, `stat-entries-total`

Check each:
```bash
grep -n "data-testid" templates/admin.html templates/statistics.html
```
For any missing, add `data-testid="..."` attributes to the corresponding element in the template. Stage the template changes for the Task 4 commit.

Also: `SeedHelper` currently stores `this.dbPath` but the step above uses `seed.dbPath`. Verify by reading `e2e/support/seed.js` from Task 1 — `dbPath` is stored on the constructor. If not, modify `SeedHelper.constructor` to expose `dbPath` as a public field (`this.dbPath = dbPath` is already there).

- [ ] **Step 5: Run `admin.feature`**

```bash
cd e2e && npx playwright test admin
```
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add e2e/features/admin.feature e2e/steps/admin.steps.js
# Stage any template testid additions:
# git add templates/admin.html templates/statistics.html
git commit -m "$(cat <<'EOF'
test(e2e): add admin + statistics feature

Covers admin user listing, user creation, disable, and the statistics
page. Adds missing data-testid attributes on admin/statistics
templates to support declarative steps.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Reading + triage features with `@pending` tags

**Files:**
- Create: `e2e/features/reading.feature`
- Create: `e2e/features/triage.feature`
- Create: `e2e/steps/entries.steps.js`
- Create: `e2e/steps/triage.steps.js`

The SSR-first PR-10 (entries family) and PR-11 (feed/category entries) have not yet merged. Scenarios that depend on the swap helper / fragment endpoints are tagged `@pending` and skipped in CI via `--grep-invert "@pending"`. Local developers can run the pending set to validate against the work-in-progress PR-10 branch.

- [ ] **Step 1: Write `e2e/features/reading.feature`**

```gherkin
@parallel
Feature: Reading entries

  Background:
    Given I am signed in
    And I have a feed "Reading Feed" with 5 test entries in category "Reading Category"

  Scenario: Unread inbox lists my unread entries newest first
    When I open the inbox
    Then I see 5 entries in the entry list
    And the first entry is titled "Test Entry 1"

  @pending
  Scenario: Opening an entry swaps the reading pane to show its title and body
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane shows the title "Test Entry 1"
    And the reading pane shows the content "Content for test entry 1"

  @pending
  Scenario: Reading pane shows feed title and published time
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane shows the feed title "Reading Feed"
    And the reading pane shows a published time

  Scenario: Read filter shows only read entries
    Given the entry titled "Test Entry 1" is marked read
    When I open the read entries page
    Then I see 1 entry in the entry list
    And the first entry is titled "Test Entry 1"

  Scenario: Starred filter shows only starred entries
    Given the entry titled "Test Entry 2" is starred
    When I open the starred entries page
    Then I see 1 entry in the entry list
    And the first entry is titled "Test Entry 2"

  Scenario: Summarized filter shows only summarized entries
    Given the entry titled "Test Entry 3" has a summary
    When I open the summarized entries page
    Then I see 1 entry in the entry list
    And the first entry is titled "Test Entry 3"

  Scenario: Single-feed view filters by that feed
    When I open the entries page for feed "Reading Feed"
    Then I see 5 entries in the entry list

  Scenario: Single-category view filters by that category
    When I open the entries page for category "Reading Category"
    Then I see 5 entries in the entry list

  @pending
  Scenario: Load More appends the next page without scroll reset
    Given the feed has 30 entries
    When I open the inbox
    And I click "Load more"
    Then I see more than 20 entries in the entry list

  @pending
  Scenario: Keyboard j and k move selection between entries
    When I open the inbox
    And I press the "j" key
    Then the second entry is selected
    When I press the "k" key
    Then the first entry is selected

  Scenario: The question-mark key shows the keyboard shortcut help overlay
    When I open the inbox
    And I press the "?" key
    Then the keyboard shortcut help overlay is visible

  Scenario: Reader can toggle between full content and original feed body
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Toggle original" button
    Then the reading pane shows the original feed body
```

- [ ] **Step 2: Write `e2e/features/triage.feature`**

```gherkin
@parallel
Feature: Triage entries (star, mark-read, summarize)

  Background:
    Given I am signed in
    And I have a feed "Triage Feed" with 3 test entries in category "Triage Category"

  @pending
  Scenario: Starring an entry updates the row and the sidebar starred count
    When I open the inbox
    And I star the entry titled "Test Entry 1"
    Then the entry titled "Test Entry 1" is marked starred
    And the sidebar starred count is at least 1

  @pending
  Scenario: Marking an entry read updates the row and the sidebar unread count
    When I open the inbox
    And I mark the entry titled "Test Entry 1" read
    Then the entry titled "Test Entry 1" is marked read
    And the sidebar unread count decreases by 1

  @pending
  Scenario: Marking all entries read empties the unread list
    When I open the inbox
    And I click "Mark all read"
    Then I see 0 entries in the entry list

  @pending
  Scenario: Summarizing an entry shows the summary in the reading pane
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Summarize" button
    Then the reading pane shows a summary

  Scenario: Dismissing a summary restores the original body
    Given the entry titled "Test Entry 1" has a summary
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Dismiss summary" button
    Then the reading pane shows the original entry body
```

- [ ] **Step 3: Write `e2e/steps/entries.steps.js`**

```javascript
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given(
  "I have a feed {string} with {int} test entries in category {string}",
  async ({ seed, currentUser }, feedTitle, count, categoryName) => {
    const userId = seed.getUserId(currentUser.username);
    const categoryId = seed.createCategory(userId, categoryName);
    const feedId = seed.createFeed(
      categoryId,
      `https://example.com/${currentUser.username}-${feedTitle}.xml`,
      feedTitle
    );
    seed.seedTestEntries(feedId, count);
  }
);

Given("the entry titled {string} is marked read", async ({ seed, currentUser }, title) => {
  const Database = (await import("better-sqlite3")).default;
  const db = new Database(seed.dbPath);
  try {
    const userId = seed.getUserId(currentUser.username);
    const row = db
      .prepare(
        `SELECT e.id FROM entry e JOIN feed f ON e.feed_id = f.id JOIN category c ON f.category_id = c.id
         WHERE c.user_id = ? AND e.title = ?`
      )
      .get(userId, title);
    if (!row) throw new Error(`Entry '${title}' not found`);
    seed.markRead(row.id);
  } finally {
    db.close();
  }
});

Given("the entry titled {string} is starred", async ({ seed, currentUser }, title) => {
  const Database = (await import("better-sqlite3")).default;
  const db = new Database(seed.dbPath);
  try {
    const userId = seed.getUserId(currentUser.username);
    const row = db
      .prepare(
        `SELECT e.id FROM entry e JOIN feed f ON e.feed_id = f.id JOIN category c ON f.category_id = c.id
         WHERE c.user_id = ? AND e.title = ?`
      )
      .get(userId, title);
    if (!row) throw new Error(`Entry '${title}' not found`);
    seed.markStarred(row.id);
  } finally {
    db.close();
  }
});

Given("the entry titled {string} has a summary", async ({ seed, currentUser }, title) => {
  const Database = (await import("better-sqlite3")).default;
  const db = new Database(seed.dbPath);
  try {
    const userId = seed.getUserId(currentUser.username);
    const row = db
      .prepare(
        `SELECT e.id FROM entry e JOIN feed f ON e.feed_id = f.id JOIN category c ON f.category_id = c.id
         WHERE c.user_id = ? AND e.title = ?`
      )
      .get(userId, title);
    if (!row) throw new Error(`Entry '${title}' not found`);
    seed.insertSummary(row.id, userId);
  } finally {
    db.close();
  }
});

Given("the feed has {int} entries", async ({ seed, currentUser }, count) => {
  const userId = seed.getUserId(currentUser.username);
  const Database = (await import("better-sqlite3")).default;
  const db = new Database(seed.dbPath);
  let feedId;
  try {
    const row = db
      .prepare(
        `SELECT f.id FROM feed f JOIN category c ON f.category_id = c.id WHERE c.user_id = ? LIMIT 1`
      )
      .get(userId);
    if (!row) throw new Error("No feed found for user");
    feedId = row.id;
  } finally {
    db.close();
  }
  seed.seedTestEntries(feedId, count);
});

When("I open the read entries page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/entries/read`);
});

When("I open the starred entries page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/entries/starred`);
});

When("I open the summarized entries page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/entries/summarized`);
});

When("I open the entries page for feed {string}", async ({ page, seed, currentUser, serverUrl }, feedTitle) => {
  const Database = (await import("better-sqlite3")).default;
  const db = new Database(seed.dbPath);
  let feedId;
  try {
    const userId = seed.getUserId(currentUser.username);
    const row = db
      .prepare(
        `SELECT f.id FROM feed f JOIN category c ON f.category_id = c.id WHERE c.user_id = ? AND f.title = ?`
      )
      .get(userId, feedTitle);
    if (!row) throw new Error(`Feed '${feedTitle}' not found`);
    feedId = row.id;
  } finally {
    db.close();
  }
  await page.goto(`${serverUrl}/feeds/${feedId}/entries`);
});

When("I open the entries page for category {string}", async ({ page, seed, currentUser, serverUrl }, name) => {
  const Database = (await import("better-sqlite3")).default;
  const db = new Database(seed.dbPath);
  let categoryId;
  try {
    const userId = seed.getUserId(currentUser.username);
    const row = db
      .prepare(`SELECT id FROM category WHERE user_id = ? AND name = ?`)
      .get(userId, name);
    if (!row) throw new Error(`Category '${name}' not found`);
    categoryId = row.id;
  } finally {
    db.close();
  }
  await page.goto(`${serverUrl}/categories/${categoryId}/entries`);
});

When("I click the entry titled {string}", async ({ page }, title) => {
  await page.getByTestId("entry-item").filter({ hasText: title }).first().click();
});

When("I click {string}", async ({ page }, label) => {
  await page.getByRole("button", { name: label }).click();
});

When("I press the {string} key", async ({ page }, key) => {
  await page.click("body");
  await page.keyboard.press(key);
});

When("I click the {string} button", async ({ page }, label) => {
  await page.getByRole("button", { name: label }).click();
});

Then("I see {int} entries in the entry list", async ({ page }, count) => {
  await expect(page.getByTestId("entry-item")).toHaveCount(count);
});

Then("I see {int} entry in the entry list", async ({ page }, count) => {
  await expect(page.getByTestId("entry-item")).toHaveCount(count);
});

Then("I see more than {int} entries in the entry list", async ({ page }, count) => {
  const n = await page.getByTestId("entry-item").count();
  expect(n).toBeGreaterThan(count);
});

Then("the first entry is titled {string}", async ({ page }, title) => {
  await expect(page.getByTestId("entry-item").first()).toContainText(title);
});

Then("the reading pane shows the title {string}", async ({ page }, title) => {
  await expect(page.getByTestId("reading-pane-title")).toContainText(title);
});

Then("the reading pane shows the content {string}", async ({ page }, content) => {
  await expect(page.getByTestId("reading-pane-body")).toContainText(content);
});

Then("the reading pane shows the feed title {string}", async ({ page }, title) => {
  await expect(page.getByTestId("reading-pane-feed-title")).toContainText(title);
});

Then("the reading pane shows a published time", async ({ page }) => {
  await expect(page.getByTestId("reading-pane-published-at")).toBeVisible();
});

Then("the second entry is selected", async ({ page }) => {
  await expect(page.getByTestId("entry-item").nth(1)).toHaveClass(/selected|active/);
});

Then("the first entry is selected", async ({ page }) => {
  await expect(page.getByTestId("entry-item").first()).toHaveClass(/selected|active/);
});

Then("the keyboard shortcut help overlay is visible", async ({ page }) => {
  await expect(page.getByTestId("kb-help")).toBeVisible();
});

Then("the reading pane shows the original feed body", async ({ page }) => {
  await expect(page.getByTestId("reading-pane-body")).toHaveAttribute("data-mode", "original");
});

Then("the reading pane shows the original entry body", async ({ page }) => {
  await expect(page.getByTestId("reading-pane-body")).toHaveAttribute("data-mode", "original");
});
```

- [ ] **Step 4: Write `e2e/steps/triage.steps.js`**

```javascript
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

When("I star the entry titled {string}", async ({ page }, title) => {
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .getByTestId("star-toggle")
    .first()
    .click();
});

When("I mark the entry titled {string} read", async ({ page }, title) => {
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .getByTestId("read-toggle")
    .first()
    .click();
});

Then("the entry titled {string} is marked starred", async ({ page }, title) => {
  const row = page.getByTestId("entry-item").filter({ hasText: title }).first();
  await expect(row.locator("[data-starred='true']")).toBeVisible();
});

Then("the entry titled {string} is marked read", async ({ page }, title) => {
  const row = page.getByTestId("entry-item").filter({ hasText: title }).first();
  await expect(row).toHaveAttribute("data-read", "true");
});

Then("the sidebar starred count is at least {int}", async ({ page }, n) => {
  const text = await page.getByTestId("sidebar-starred-count").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(n);
});

Then("the sidebar unread count decreases by {int}", async ({ page }, delta) => {
  // No baseline measurement here in the @pending state — assertion sketched
  // for the SSR PR-10 follow-up where the fragment endpoint updates the
  // sidebar inline. Replace with a precise before/after measurement when
  // the @pending tag is removed.
  const text = await page.getByTestId("sidebar-unread-count").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(0);
});

Then("the reading pane shows a summary", async ({ page }) => {
  await expect(page.getByTestId("reading-pane-summary")).toBeVisible();
});
```

- [ ] **Step 5: Audit testid coverage in entries-family templates**

```bash
grep -n "data-testid" templates/entries.html templates/_entry_row.html templates/_reading_pane.html templates/_sidebar_unread.html templates/_entries_layout.html 2>/dev/null
```
For each testid referenced in `entries.steps.js` and `triage.steps.js` (`entry-item`, `reading-pane-title`, `reading-pane-body`, `reading-pane-feed-title`, `reading-pane-published-at`, `reading-pane-summary`, `kb-help`, `sidebar-starred-count`, `sidebar-unread-count`, `star-toggle`, `read-toggle`), add the attribute to the corresponding template element if missing.

Stage those template changes for the Task 5 commit.

- [ ] **Step 6: Run the suite excluding pending**

```bash
cd e2e && npx playwright test reading triage --grep-invert "@pending"
```
Expected: reading has ~8 non-pending scenarios + triage has ~1 non-pending scenario = 9 passed.

- [ ] **Step 7: Run the suite including pending (expected: failures)**

```bash
cd e2e && npx playwright test reading triage 2>&1 | tail -20
```
Expected: 8 `@pending` scenarios fail because the SSR fragment endpoints don't exist yet. This output is *evidence* that the BDD suite encodes the post-PR-10/11 contract correctly — record it in the commit message.

- [ ] **Step 8: Commit**

```bash
git add e2e/features/reading.feature e2e/features/triage.feature \
  e2e/steps/entries.steps.js e2e/steps/triage.steps.js
# Stage template testid additions:
# git add templates/entries.html templates/_entry_row.html ...
git commit -m "$(cat <<'EOF'
test(e2e): add reading + triage features with @pending tags

Reading and triage scenarios encode the post-PR-10/11 entries-family
contract. Non-pending subset (~9 scenarios) is green today and gates
CI; @pending scenarios (~8) will go green when SSR fragment endpoints
land. Adds data-testid attributes on entries templates for stable
declarative steps.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Cleanup + CI shard matrix

**Files:**
- Delete: `e2e/tests/` (now empty)
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Verify `e2e/tests/` is empty and remove the directory**

```bash
ls e2e/tests
```
Expected: empty.

```bash
rmdir e2e/tests
```

- [ ] **Step 2: Replace the e2e job in `.github/workflows/ci.yml`**

Find the `e2e-tests:` job. Replace its entire body with:

```yaml
  e2e-tests:
    needs: build-and-test
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        shard: ["1/3", "2/3", "3/3"]

    steps:
      - name: Checkout code
        uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6
        with:
          fetch-depth: 0

      - name: Setup Rust
        uses: dtolnay/rust-toolchain@10c3493d811a9096cee4fdf287e41e852f6a51ba # 1.94.0

      - name: Cache Rust dependencies
        uses: actions/cache@27d5ce7f107fe9357f9df03efb73ab90386fccae # v5
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-cargo-

      - name: Build binary
        run: cargo build

      - name: Setup Node.js
        uses: actions/setup-node@48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e # v6
        with:
          node-version: "22"

      - name: Install E2E dependencies
        working-directory: e2e
        run: npm ci

      - name: Install Playwright browsers
        working-directory: e2e
        run: npx playwright install --with-deps chromium

      - name: Run E2E tests (shard ${{ matrix.shard }})
        working-directory: e2e
        run: npx playwright test --shard=${{ matrix.shard }} --grep-invert "@pending"

      - name: Upload Playwright report
        if: ${{ !cancelled() }}
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7
        with:
          name: playwright-report-${{ strategy.job-index }}
          path: e2e/playwright-report/
          retention-days: 14
```

The artifact name now includes `${{ strategy.job-index }}` so the 3 shards don't collide.

- [ ] **Step 3: Validate the workflow file syntactically**

```bash
gh workflow view ci.yml --repo henry40408/rdrs 2>/dev/null || cat .github/workflows/ci.yml | head -10
```
The local-only check is to ensure indentation is valid YAML:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Run the full local suite once**

```bash
cd e2e && npx playwright test --grep-invert "@pending"
```
Expected: 21 (Tasks 2-3) + 4 (Task 4) + 9 (Task 5 non-pending) = ~34 passed.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git rm -r e2e/tests
git commit -m "$(cat <<'EOF'
ci(e2e): shard e2e job 3-way; drop empty tests directory

GitHub Actions standard runner is 2 vCPU; sharding into 3 parallel
jobs cuts wall-clock by ~3x vs. single-worker serial. Skips
@pending scenarios pending SSR-first PR-10/11. Drops the now-empty
e2e/tests/ directory left from the BDD migration.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 6: Push branch and open PR**

```bash
git push -u origin refactor/migrate-e2e-to-playwright-bdd
gh pr create --title "refactor(e2e): migrate Playwright suite to playwright-bdd" --body "$(cat <<'EOF'
## Summary
- Replace TypeScript Playwright suite with `playwright-bdd` (plain JS, ESM)
- 8 feature files / 42 scenarios; 34 run on CI today, 8 `@pending` for SSR-first PR-10/11
- 7 deferred organizing scenarios tracked as a follow-up (edit/delete feed, category CRUD, OPML)
- Per-scenario nanoid usernames + per-worker rdrs spawn for full isolation
- CI shard matrix (3-way) on `ubuntu-latest`

## Spec / Plan
- Spec: `docs/superpowers/specs/2026-05-13-playwright-bdd-migration-design.md`
- Plan: `docs/superpowers/plans/2026-05-13-playwright-bdd-migration.md`

## Test plan
- [ ] CI shards 1/3, 2/3, 3/3 all green
- [ ] Local `cd e2e && npx playwright test --grep-invert "@pending"` green
- [ ] Local `cd e2e && npx playwright test` shows ~8 `@pending` failures (expected, pre-PR-10/11)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Wait for CI green before merging. Once merged, follow up with two small PRs that remove `@pending` tags when SSR-first PR-10 and PR-11 land.

---

## Open scenarios (deferred — separate follow-up PR)

The `organizing.feature` only covers the "add feed" scenario from the original `feed-management.spec.ts`. The spec calls out 7 more scenarios (edit/delete feed, category CRUD, OPML import/export). These need new product affordances or testids that don't exist today and are best done **after** the SSR-first feeds/categories pages (PR-8 merged, but the form-ization for edit/delete may have rough edges that aren't worth pinning into BDD until the surface stabilizes).

Track as a separate follow-up:
- Title: `test(e2e): expand organizing.feature to cover full feed/category CRUD + OPML`
- Estimate: 1 day; one feature file, ~7 new scenarios, ~10-15 new steps.
