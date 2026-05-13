# Playwright-BDD migration

**Status:** design / approval pending
**Date:** 2026-05-13
**Predecessors:** `2026-05-08-ssr-first-redesign-design.md`

## Background

The SSR-first redesign (PRs #185-#192, #194; PR-9 through PR-12 still
ahead) keeps regressing user-facing behavior between merges. The
existing 5 Playwright TypeScript specs (`auth`, `feed-management`,
`responsive`, `search`, `theme`) describe behavior in terms of
selectors and URLs, so every SSR/CSR move ripples through the spec
files themselves — they do not anchor a stable contract.

We are replacing the existing e2e suite with a `playwright-bdd`
suite written in plain JavaScript. Scenarios are written in
declarative Gherkin from the user's perspective; selectors and URLs
live only in step definitions. The migration's primary goal is to
make user behavior the load-bearing artifact so subsequent SSR
rewrites can be driven by "make the BDD turn green" rather than
"hope nothing broke."

## Goals

- Replace `e2e/tests/*.spec.ts` with `e2e/features/*.feature` +
  `e2e/steps/*.js`. All 5 existing specs translated; new coverage
  added for the entries family (reading/triage) and admin pages.
- All test code is plain JavaScript (ESM). No TypeScript, no build
  step beyond `bddgen` (the playwright-bdd feature-to-spec compiler).
- Scenarios written declaratively: feature files do not contain
  selectors, URLs, or click coordinates.
- Test execution scales with available cores. Local 8-core dev box
  finishes in <30s; CI completes in <60s wall-clock via shard matrix.
- Each scenario is fully isolated by a unique nanoid-suffixed user,
  eliminating the `OR IGNORE` register-twice patterns in the current
  fixtures.

## Non-goals

- WebAuthn / passkey scenarios. Virtual authenticator setup is out
  of scope for this migration; passkey JSON endpoints stay covered
  by Rust integration tests, UI by manual smoke.
- Service Worker / offline behavior (not in the product).
- Visual regression / screenshot diffing. `e2e/scripts/screenshots.ts`
  is kept as a JS-converted utility but is not part of the test
  contract.
- Cucumber/Cypress migration. We commit to `playwright-bdd` on top
  of `@playwright/test`.
- Coverage of the GReader API (`/reader/api/0/*`) — external contract,
  covered by Rust integration tests.

## Architecture

### Stack

- `@playwright/test` v1.50+ (already in the project)
- `playwright-bdd` v8+ — feature compiler + `createBdd(test)` helper
- `better-sqlite3` v11 — direct DB seed (already in the project)
- `nanoid` v5 — per-scenario unique usernames
- Plain JS, ESM (`"type": "module"` in `e2e/package.json`)
- Node 22+ (matches CI image)

No TypeScript, no esbuild, no bundling. `bddgen` is the only
preprocessing step and writes generated `.spec.js` files into
`.features-gen/` (gitignored).

### Directory layout

```
e2e/
  package.json              # type: module
  playwright.config.js      # testDir = ".features-gen"
  bddgen.config.js          # features → .features-gen mapping
  global-setup.js           # builds debug rdrs binary if missing
  .gitignore                # adds .features-gen/

  features/                 # user-facing contract
    authentication.feature
    reading.feature
    organizing.feature
    triage.feature
    search.feature
    preferences.feature
    admin.feature
    responsive.feature

  steps/                    # step definitions, one per capability
    auth.steps.js
    navigation.steps.js
    entries.steps.js
    triage.steps.js
    organize.steps.js
    search.steps.js
    preferences.steps.js
    admin.steps.js
    responsive.steps.js

  support/                  # fixtures and helpers
    fixtures.js             # extend(base, { currentUser, api, seed, ... })
    server.js               # per-worker rdrs binary spawn (was fixtures/rdrs.ts)
    api.js                  # HTTP helper (was helpers/api.ts)
    seed.js                 # direct SQLite seed (was helpers/seed.ts)

  scripts/
    screenshots.js          # converted to JS, kept as a utility, not a test
```

Deleted at migration end: `e2e/tests/`, `e2e/fixtures/rdrs.ts`,
`e2e/helpers/*.ts`, `e2e/tsconfig.json`.

### Fixture model

```javascript
// support/fixtures.js
import { test as base } from "playwright-bdd";
import { customAlphabet } from "nanoid";

const nano = customAlphabet("abcdefghijklmnopqrstuvwxyz0123456789", 8);

export const test = base.extend({
  serverUrl: [serverUrlFixture, { scope: "worker" }],
  dbPath:    [dbPathFixture,    { scope: "worker" }],
  api:       [apiFixture,       { scope: "worker" }],
  seed:      [seedFixture,      { scope: "worker" }],

  currentUser: async ({}, use) => {
    await use({ username: `e2e-${nano()}`, password: "password123" });
  },
});

export const expect = base.expect;
```

`currentUser` is the central isolation primitive: every scenario
receives a fresh username, so scenarios sharing a worker (and its
SQLite file) never collide.

### Step style

Declarative, user-language. Selectors and URLs are an implementation
detail of step definitions, not the feature.

```gherkin
# features/authentication.feature
Feature: Authentication

  Scenario: New user can register, sign in, and reach the inbox
    Given I am a new user
    When I register with matching passwords
    Then I am redirected to the login page with a success message
    When I sign in with my credentials
    Then I land on the unread inbox
```

```javascript
// steps/auth.steps.js
import { createBdd } from "playwright-bdd";
import { test } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("I am signed in", async ({ page, currentUser, api, serverUrl }) => {
  await api.register(currentUser.username, currentUser.password);
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
});
```

### Tags

- `@mobile` / `@tablet` / `@desktop` — viewport selection in
  `responsive.feature`; the step `Given I am viewing on <viewport>`
  applies `page.setViewportSize`.
- `@pending` — scenarios that depend on SSR PR-10/11 fragment
  endpoints. CI runs with `--grep-invert "@pending"` until those
  PRs land. Local developers can opt in by removing the filter.

## Scenario inventory

Mapped to the 13 SSR-first routes from
`2026-05-08-ssr-first-redesign-design.md`.

### `authentication.feature` (`/login`, `/register`)
Source: `auth.spec.ts` (3 scenarios), plus 1 new redirect check.

- New user can register, sign in, and reach the unread inbox
- Sign-in with the wrong password shows an error
- Mismatched passwords on registration show a client-side error
- Authenticated user visiting `/login` is redirected to the inbox

### `reading.feature` (`/`, `/entries`, `/entries/{read,starred,summarized}`, `/feeds/{id}/entries`, `/categories/{id}/entries`)
Source: all new.

- Unread inbox lists my unread entries newest first
- Opening an entry swaps the reading pane to show its title and body `@pending`
- Reading pane shows feed title and published time `@pending`
- `/entries/read` shows only read entries
- `/entries/starred` shows only starred entries
- `/entries/summarized` shows only summarized entries
- `/feeds/{id}/entries` filters by a single feed
- `/categories/{id}/entries` filters by a single category
- Load More appends the next page without scroll reset `@pending`
- Keyboard `j`/`k` moves selection between entries `@pending`
- `?` shows the keyboard shortcut help overlay
- Toggle between full content and original feed body (per PR #203)

### `triage.feature` (POST endpoints: `*/star`, `*/read`, `*/summarize`)
Source: all new.

- Starring an entry updates the row and the sidebar starred count `@pending`
- Marking an entry read updates the row and the sidebar unread count `@pending`
- Marking all entries read empties the unread list `@pending`
- Summarizing an entry shows the summary in the reading pane `@pending`
- Dismissing a summary restores the original body (per PR #204)

### `organizing.feature` (`/feeds`, `/categories`)
Source: `feed-management.spec.ts` (1 scenario), expanded.

- Adding a feed makes it appear in the feeds table
- Editing a feed updates its title in the table
- Deleting a feed removes it from the table
- Adding a category makes it appear under "All categories"
- Renaming a category updates it everywhere
- Deleting a category removes its feeds from the sidebar
- Importing OPML creates the feeds listed in the file
- Exporting OPML produces a downloadable file

### `search.feature` (`/search`)
Source: `search.spec.ts` (2 scenarios), plus 1 empty-state.

- Searching for a term shows matching entries
- Pressing `/` focuses the search input
- Searching for a term with no matches shows an empty state

### `preferences.feature` (`/user-settings`, `/settings`)
Source: `theme.spec.ts` (3 scenarios), plus account scenarios.

- Switching to dark theme sets `data-theme="dark"`
- Switching to light theme sets `data-theme="light"`
- Switching to system theme removes the `data-theme` attribute
- Changing my password lets me sign in with the new password
- Changing my display name updates the navbar greeting

### `admin.feature` (`/admin`, `/statistics`)
Source: all new.

- Admin sees the list of all users
- Admin creates a new user account
- Admin disables a user account
- Statistics page shows feed and entry counts for the signed-in user

### `responsive.feature` (viewport-cutting behavior)
Source: `responsive.spec.ts` (6 scenarios), reorganized by viewport.

- `@mobile` Sidebar is hidden by default and toggled by the hamburger
- `@mobile` Entry list is full-width single column
- `@mobile` Categories table renders as cards
- `@tablet` Sidebar is a drawer (not always-visible)
- `@tablet` Tables keep table layout (not card)
- `@tablet` Entry list is full-width single column
- `@desktop` Sidebar is always visible
- `@desktop` Reading pane sits beside the entry list

**Totals.** 8 feature files, ~45 scenarios. ~12 are `@pending`
(reading/triage scenarios blocked on SSR-first PR-10/11), ~33 run
on CI today.

## Parallelism

### Isolation invariants (already true)
- Per-worker rdrs binary spawn into a per-worker tempdir SQLite file
  → worker-level isolation, no shared server state.
- Per-scenario `currentUser` nanoid → scenario-level isolation
  within a worker; concurrent scenarios on the same worker DB are
  safe because all writes are scoped to the unique username.
- SQLite WAL mode (default) → concurrent reader/writer scenarios
  within a worker do not deadlock.

### Local
```javascript
// playwright.config.js
export default defineConfig({
  testDir: ".features-gen",
  fullyParallel: true,
  workers: process.env.CI ? "50%" : "75%",
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? "html" : "list",
  // ...
});
```
On an 8-core dev machine, `"75%"` → 6 workers; ~45 scenarios at
~3s each finish in **~20-25s wall-clock** including 6 rdrs spawn
warmups.

### File-internal parallelism
Each `.feature` compiles to one `.spec.js`. Playwright defaults to
serial execution within a file. We override with `describeMode:
"parallel"` in `bddgen.config.js` so scenarios inside the same
feature also distribute across workers.

```javascript
// bddgen.config.js
export default {
  features: "features/*.feature",
  steps: "steps/*.js support/fixtures.js",
  outputDir: ".features-gen",
  describeMode: "parallel",
};
```

### CI sharding
GitHub Actions standard runners are 2 vCPU. A single job with many
workers competes for CPU with rdrs spawn. Use shard matrix instead:

```yaml
# .github/workflows/e2e.yml
jobs:
  e2e:
    strategy:
      fail-fast: false
      matrix:
        shard: ["1/3", "2/3", "3/3"]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/cache@v4
        with:
          path: |
            ~/.cargo
            target
          key: e2e-${{ hashFiles('Cargo.lock') }}
      - run: cargo build
      - run: cd e2e && npm ci
      - run: cd e2e && npx bddgen
      - run: cd e2e && npx playwright test --shard=${{ matrix.shard }} --grep-invert "@pending"
```

- 8 feature files / 3 shards ≈ 2-3 features per shard
- `workers: "50%"` on 2 vCPU = 1 worker per shard
- Three shards run in parallel; each shard finishes in ~30-45s
- `--grep-invert "@pending"` skips SSR-PR-10/11-blocked scenarios

### Performance summary
| Environment | Workers | Wall-clock |
|---|---|---|
| Local (8c) | 6 | ~25s |
| Local (16c) | 12 | ~12s |
| CI single job (2 vCPU) | 1 | ~90s |
| CI 3-shard (2 vCPU × 3) | 1 × 3 parallel | ~30-45s |

### Shard balance risk
Playwright shards by test count, not duration. `responsive.feature`
scenarios are slower (viewport setup), `@pending`-heavy features
are skipped fast. If observed shard imbalance > 30%, split
`reading.feature` or move responsive scenarios into smaller files.

## Migration plan

Six PRs, each independently mergeable. Between PRs the repo is in
a consistent state (old specs still run on `main`; new BDD suite
gates the same CI job once landed).

| # | Scope | Risk |
|---|---|---|
| 1 | **Skeleton.** Add `playwright-bdd`, `nanoid`. Convert `e2e/package.json` to ESM. Port `helpers/api.ts`, `helpers/seed.ts`, `fixtures/rdrs.ts`, `global-setup.ts` to JS. Add `support/fixtures.js` with `currentUser`. New `playwright.config.js`, `bddgen.config.js`. Old `tests/*.spec.ts` still present and still run. | Low |
| 2 | **First feature** (`authentication.feature`). Steps for register/login/redirect. Verify `bddgen` + `describeMode: parallel` works locally and on CI. | Low |
| 3 | **Static-shape features** — `preferences`, `search`, `responsive`, `organizing`. Translate the 4 existing specs. Delete corresponding `*.spec.ts` files. | Medium |
| 4 | **Admin + statistics** features. | Low |
| 5 | **Reading + triage** features with `@pending` tags on SSR-dependent scenarios. CI runs the non-`@pending` subset. | Medium |
| 6 | **Cleanup.** Delete `e2e/tsconfig.json`. Update `.github/workflows/e2e.yml` to the 3-shard matrix. Update the e2e README (if any) and `CLAUDE.md` references. | Low |

When SSR-first PR-10 lands, a follow-up PR removes the `@pending`
tags on the reading scenarios; PR-11 removes them on the triage
scenarios.

## Open questions

None at design time. Step-definition ergonomics (parameter passing,
table conversion) and exact `bddgen` config tuning surface during
PR 1-2 and are answered there.
