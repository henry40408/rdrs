# CLAUDE.md

Guidance for Claude Code (claude.ai/code) in this repository.

## Commands

### Go (repo root)

The module is `github.com/henry40408/rdrs`; `go.mod`'s `toolchain` line is the
single source of the Go version. Dependencies are vendored: pass `-mod=vendor`
when a command should not touch the network (CI does).

- Format (CI gate): `gofmt -l` over the tracked `.go` files outside `vendor/`
  must print nothing. Run `gofmt -w` before committing.
- Lint (CI gate): `go vet -mod=vendor ./...` and
  `go run honnef.co/go/tools/cmd/staticcheck@v0.8.2 ./...`. `staticcheck.conf`
  holds the disabled checks and the reason for each.
- Supply chain (CI gate, job `deps`): `go mod tidy -diff`, `go mod vendor` leaves
  no diff, `govulncheck`, and `go-licenses` against the allow-list in
  `.github/workflows/ci.yml` (Apache-2.0, BSD-2/3-Clause, ISC, MIT, MIT-0, Zlib,
  Unlicense, CC0-1.0, Unicode-3.0, BSL-1.0; `e2e/` also allows MPL-2.0). A new
  dependency must be vendored and committed.
- Build: `go build -o bin/rdrs ./cmd/rdrs`. The version comes from
  `-ldflags "-X github.com/henry40408/rdrs/internal/version.version=$V"`; without
  it `internal/version` falls back to the VCS stamp in the build info, then
  `unknown` (Docker builds pass `GIT_VERSION` as a build arg because
  `.dockerignore` excludes `.git`).
- Release: `gh release create --generate-notes`; never add a `CHANGELOG.md`.
- Test: `go test -race ./...`; one test: `go test -run TestLoginSuccess ./internal/server`.
  `RDRS_FAST_HASH=1` uses minimal-cost Argon2 for fast local tests. **Never**
  set it in production.
- PostgreSQL: with `TEST_DATABASE_URL=postgres://user:pass@host/db` set, the
  database tests also run against PostgreSQL (CI job `integration`, service
  `postgres:17-alpine`); packages share one database, so add `-p 1`. Unset, they
  run on SQLite only.
- Coverage (as CI): `RDRS_FAST_HASH=1 go test -coverprofile=coverage.out ./...`.
- Mutation (CI gate): `go run github.com/go-gremlins/gremlins/cmd/gremlins@v0.6.0 unleash .`
  after `go clean -testcache`. The efficacy threshold lives in `.gremlins.yaml`;
  gremlins v0.6.0 ignores `--threshold-efficacy` on the command line, so never
  pass it.
- Benchmarks: `go test -run '^$' -bench . -benchmem -count 5 ./internal/server ./internal/sanitize`.
  `BenchmarkEntryPages` (`internal/server/bench_test.go`) measures every list
  route; compare before and after with `benchstat`. Raise `RDRS_BENCH_ENTRIES`
  for realistic data volume.
- Golden files: `internal/server/golden_test.go` pins full pages captured from
  the original Rust server (`testdata/html/`, `testdata/vectors/`). Do not
  regenerate them to make a test pass; a diff is a behaviour change.

### E2E (godog + chromedp, from `e2e/`)

`e2e/` is a **separate Go module** (own `go.mod` and `vendor/`) so the browser
and Gherkin dependencies stay out of the shipped binary. Run `go vet` and
`staticcheck` inside it too; CI does.

- Build the server first: `go build -o bin/rdrs ./cmd/rdrs`. The harness reuses
  `bin/rdrs` when it exists, so **rebuild after editing `static/` or Go
  source** (assets are embedded at compile time).
- Run all: `cd e2e && go test -mod=vendor -count=1 -timeout 25m ./...`. One
  feature: `E2E_FEATURES=features/reading.feature go test -mod=vendor ./`.
  `E2E_CHROME=/path/to/chrome` pins the browser. Scenarios run one at a time,
  each with its own server and browser.
- A browser must be installed (chromedp starts it directly; no driver).
  macOS: `brew install --cask ungoogled-chromium`; CI's `ubuntu-latest` has Chrome.
- CSP audit (CI gate): `cd e2e && go run -mod=vendor ./cmd/cspaudit` — fails on
  any browser CSP violation. Run after touching `internal/views/`,
  `static/css/` or `static/js/` (the source scan in the security-headers
  middleware can't see runtime markup or shadow DOM).
- No-JS walkthrough (CI gate): `cd e2e && go run -mod=vendor ./cmd/nojs` —
  scripts disabled and every `*.js` request aborted.
- Touch-target report (not a gate): `go run -mod=vendor ./cmd/touchaudit`.
- README screenshots: `go run -mod=vendor ./cmd/screenshots` (writes `../screenshots/`).
- Swap benchmark (`dom-bench`): not ported. The Rust tool is
  `e2e/src/bin/dom_bench.rs` at commit `9aa9876` (`git show 9aa9876:e2e/src/bin/dom_bench.rs`);
  port it from there when browser-side swap performance needs a baseline again.

**`.feature` files are the contract**, read as-is. New scenarios need steps in
`e2e/steps_*_test.go`; undefined steps fail the suite.

## UI changes require screenshot updates

Any change to rendered UI (`static/css/`, `internal/views/`, `static/js/`):
`go build -o bin/rdrs ./cmd/rdrs`, then `cd e2e && go run -mod=vendor ./cmd/screenshots`,
and include the four updated images under `screenshots/` (referenced by
`README.md`) in the same change. The generator (`e2e/cmd/screenshots`) captures
the unread list with reading pane and the keyboard-help overlay, light and dark.

## Architecture

**views (`internal/views`) → HTTP handlers (`internal/server`) → models (`internal/models`) → SQLite/PostgreSQL (`internal/db`).**
`ARCHITECTURE.md` is the full map. Rules most easily violated:

- **SSR-first, no frontend build tooling.** Logged-in pages are server-rendered;
  mutations are form POSTs answered with flash + redirect.
  `static/js/` is progressive enhancement only — vanilla ES modules; no bundlers
  or transpilers.
- **Everything compiles into the single binary** (CSS, JS, favicons, via
  `go:embed` in `static/`) — hence the mandatory rebuild.
- **One SQL text for both backends.** Dialect forks go through `internal/db`
  (`dialect.go`) and the entry filters in `internal/models/entry`, except the
  entry upsert's NULL-safe inequality (`IS NOT` / `IS DISTINCT FROM`), a
  deliberate hand-dispatched SQLite/PostgreSQL pair, since a blanket `IS NOT`
  rewrite would corrupt every `IS NOT NULL`. Migrations:
  `migrations/{sqlite,postgres}/`. The database file stays compatible with the
  Rust releases: do not add a migration or change a stored format without a
  decision recorded in the PR.
- **Background DB work must call `db.Background()`** to yield to interactive
  SQLite writes (no-op on PostgreSQL).
- **`RDRS_SECRET` is the one root key**: session cookie, image proxy, GReader
  post token and CSRF token derive from it with domain separation in
  `internal/secret`. Session/credential lifecycle events (creation, renewal,
  token rotation, destruction, masquerade start/stop, re-authentication,
  passkey added/removed) are audited under the `rdrs::audit` log target
  (`internal/audit`), with sessions identified only by `secret.AuditID`. **A new
  credential path must emit an audit event.**
- **Logging** is `log/slog`; calls use a static message and an `event`
  attribute, with values as typed fields. The level is `RDRS_LOG_LEVEL` (`debug`, `info`, `warn`, `error`);
  there is no per-module filter. `RUST_LOG` is retired: the binary refuses to
  start while it is set.
- **GitHub Actions are pinned by commit SHA**, with the tag as a trailing
  comment (`uses: owner/action@<sha> # vX.Y.Z`).
