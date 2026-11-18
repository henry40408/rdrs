package e2e

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// The service worker: registration, offline fallback, and what its caches may
// hold. Browser-only: a request-level test cannot see CSP acceptance, page
// control or the offline fallback.

// cachedPaths lists the paths held by every cache whose name has the prefix.
func cachedPaths(w *world, prefix string) ([]string, error) {
	paths, err := harness.EvalAsync[[]string](w.browser, `(async () => {
		const paths = [];
		for (const name of await caches.keys()) {
			if (!name.startsWith(`+harness.JSLiteral(prefix)+`)) { continue; }
			const cache = await caches.open(name);
			for (const request of await cache.keys()) {
				paths.push(new URL(request.url).pathname);
			}
		}
		return paths;
	})()`)
	return paths, err
}

func registerPWASteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^a service worker controls the page$`, w.serviceWorkerControlsPage)
	sc.Step(`^the network goes offline$`, func() error { return w.browser.SetOffline(true) })
	sc.Step(`^I see the offline page$`, func() error { return w.browser.ExpectVisible("offline-page") })
	sc.Step(`^the worker's cache holds nothing but public assets$`, w.cacheHoldsOnlyPublicAssets)
	sc.Step(`^I keep (\d+) entries for offline reading$`, func(keep int) error {
		return w.setOfflineKeep(strconv.Itoa(keep))
	})
	sc.Step(`^I stop keeping entries for offline reading$`, w.stopKeepingEntries)
	sc.Step(`^my entries have been saved for offline reading$`, func() error { return w.waitForSavedEntries(3) })
	sc.Step(`^(\d+) entries have been saved for offline reading$`, w.waitForSavedEntries)
	sc.Step(`^nothing has been asked of the server that names no file$`, w.noUnservableStatic)
	sc.Step(`^every control that needs the server is disabled$`, w.serverBoundControlsDisabled)
	sc.Step(`^I open the first entry in the list$`, func() error {
		return w.browser.ClickCSS(`.entry-item a[data-swap="#reading-pane"]`)
	})
	sc.Step(`^Load More is disabled$`, func() error {
		disabled := "true"
		return w.browser.ExpectAttr("#load-more", "aria-disabled", &disabled)
	})
	sc.Step(`^opening a saved entry is still offered$`, w.openingEntryStillOffered)
	sc.Step(`^I am told the action has to wait for the connection$`, func() error {
		return w.browser.ExpectText("flash-message", "wait")
	})
	sc.Step(`^the sidebar shows the connection is up$`, w.sidebarShowsConnectionUp)
	sc.Step(`^the sidebar shows the connection is gone$`, func() error {
		return harness.Eventually("the connection lamp to report offline", func() (bool, error) {
			return w.browser.IsVisible("connection-offline")
		})
	})
	sc.Step(`^the lamp is breathing$`, w.lampIsBreathing)
	sc.Step(`^the sidebar offers the saved entries$`, func() error { return w.browser.ExpectVisible("nav-offline") })
	sc.Step(`^the sidebar does not offer the saved entries$`, w.sidebarOmitsLibrary)
	sc.Step(`^the scriptless navigation offers the saved entries$`, w.scriptlessNavOffersLibrary)
	sc.Step(`^the scriptless navigation does not offer the saved entries$`, w.scriptlessNavOmitsLibrary)
	sc.Step(`^I open the saved entries from the sidebar$`, w.openLibraryFromSidebar)
}

// serviceWorkerControlsPage: clients.claim() takes control without a reload,
// but not instantly.
func (w *world) serviceWorkerControlsPage() error {
	return harness.Eventually("a service worker to take control", func() (bool, error) {
		return harness.Eval[bool](w.browser, `Boolean(navigator.serviceWorker && navigator.serviceWorker.controller)`)
	})
}

// cacheHoldsOnlyPublicAssets: signed-in responses are no-store plus
// Vary: Cookie, which the Cache API ignores, so a cached navigation would leak
// one reader's articles to the next.
func (w *world) cacheHoldsOnlyPublicAssets() error {
	cached, err := cachedPaths(w, "")
	if err != nil {
		return err
	}
	if len(cached) == 0 {
		return fmt.Errorf("sanity: the worker is expected to have precached something")
	}
	for _, path := range cached {
		if path != "/offline" && !strings.HasPrefix(path, "/static/") {
			return fmt.Errorf("the cache holds %s, which is neither the offline page nor a public asset: %v", path, cached)
		}
	}
	return nil
}

// setOfflineKeep goes through the form, since that value decides what is
// stored on disk.
func (w *world) setOfflineKeep(keep string) error {
	if err := w.goto_("/user-settings"); err != nil {
		return err
	}
	if err := w.browser.ExpectVisible("offline-keep"); err != nil {
		return err
	}
	if err := w.browser.Fill("offline-keep", keep); err != nil {
		return err
	}
	if err := w.browser.SubmitCSS(`form[action="/user-settings/preferences"] button[type=submit]`); err != nil {
		return err
	}
	return w.expectPath("/user-settings")
}

func (w *world) stopKeepingEntries() error {
	if err := w.setOfflineKeep("0"); err != nil {
		return err
	}
	// offline.js wipes on the next load before its manifest fetch, so the
	// redirect triggers it but may not have finished it.
	return harness.Eventually("the saved entries to be dropped", func() (bool, error) {
		names, err := harness.EvalAsync[[]string](w.browser, `caches.keys().then((names) => names.filter((n) => n.startsWith('rdrs-offline-')))`)
		return len(names) == 0, err
	})
}

// waitForSavedEntries polls the cache, not the network: a fetch that was never
// stored is the failure worth catching.
func (w *world) waitForSavedEntries(count int) error {
	err := harness.Eventually("the entry list to be mirrored into the offline cache", func() (bool, error) {
		paths, err := cachedPaths(w, "rdrs-offline-")
		if err != nil {
			return false, err
		}
		// The library page is stored last, so it implies its fragments.
		// utils.js is only reachable via an import in app.js, proving the
		// walk is transitive.
		fragments := 0
		for _, path := range paths {
			if strings.HasSuffix(path, "/fragment") {
				fragments++
			}
		}
		return contains(paths, "/entries/offline") && contains(paths, "/static/js/utils.js") && fragments == count, nil
	})
	if err != nil {
		return err
	}
	// The asset walk scans source text, so a comment mentioning an import once
	// triggered a request; an extensionless cached path is that symptom.
	paths, err := cachedPaths(w, "rdrs-offline-")
	if err != nil {
		return err
	}
	for _, path := range paths {
		if strings.HasPrefix(path, "/static/") && !namesAFile(path) {
			return fmt.Errorf("%s was saved as a static asset but does not name a file", path)
		}
	}
	return nil
}

func contains(list []string, wanted string) bool {
	for _, item := range list {
		if item == wanted {
			return true
		}
	}
	return false
}

func namesAFile(path string) bool {
	name := path[strings.LastIndex(path, "/")+1:]
	return strings.Contains(name, ".") && !strings.HasSuffix(name, ".")
}

// noUnservableStatic: the asset scan cannot tell a real url() or import from a
// comment about one, which once caused a 404 on every sync. Resource timings
// include fetch(), which CDP interception patterns cannot match by "names no
// file".
func (w *world) noUnservableStatic() error {
	paths, err := harness.Eval[[]string](w.browser, `performance.getEntriesByType('resource')
		.map((e) => new URL(e.name).pathname)
		.filter((p) => p.startsWith('/static/'))`)
	if err != nil {
		return err
	}
	if len(paths) == 0 {
		return fmt.Errorf("sanity: the page is expected to have loaded some static assets")
	}
	for _, path := range paths {
		if !namesAFile(path) {
			return fmt.Errorf("the sync asked for %s, which can name no file the server has", path)
		}
	}
	return nil
}

// serverBoundControlsDisabled covers forms (including method="get"), links
// and auto-submitting selects, since offline.js allowlists what does work.
func (w *world) serverBoundControlsDisabled() error {
	// Offline is only declared once the retrying probe fails, so wait for it.
	err := harness.Eventually("the page to be marked offline", func() (bool, error) {
		return harness.Eval[bool](w.browser, `document.documentElement.hasAttribute('data-offline')`)
	})
	if err != nil {
		return err
	}
	leaked, err := harness.Eval[[]string](w.browser, `(() => {
		const WORKS_OFFLINE = 'a[data-swap="#reading-pane"], a[href="/"], a[href="/entries/offline"]';
		const SERVER_BOUND = 'form, a[href], select[data-mark-read-scope], select[data-status-select]';
		return [...document.querySelectorAll(SERVER_BOUND)]
			.filter((el) => !el.matches(WORKS_OFFLINE))
			.filter((el) => !el.hasAttribute('data-offline-disabled'))
			.map((el) => el.tagName.toLowerCase() + (el.getAttribute('action') || el.getAttribute('href') || ''));
	})()`)
	if err != nil {
		return err
	}
	if len(leaked) > 0 {
		return fmt.Errorf("these reach the server but are still live offline: %v", leaked)
	}
	return nil
}

// openingEntryStillOffered: disabling everything would be trivially correct
// and useless.
func (w *world) openingEntryStillOffered() error {
	live, err := harness.Eval[bool](w.browser, `(() => {
		const link = document.querySelector('.entry-item a[data-swap="#reading-pane"]');
		return Boolean(link) && !link.hasAttribute('data-offline-disabled');
	})()`)
	if err != nil {
		return err
	}
	if !live {
		return fmt.Errorf("no entry in the saved list can be opened")
	}
	return nil
}

// sidebarShowsConnectionUp: the word "Offline" stays in the markup, so assert
// it is display:none.
func (w *world) sidebarShowsConnectionUp() error {
	if err := w.browser.ExpectVisible("connection-status"); err != nil {
		return err
	}
	if err := w.browser.ExpectHidden("connection-offline"); err != nil {
		return err
	}
	animation, err := w.browser.ComputedStyle(".conn-dot", "animation-name")
	if err != nil {
		return err
	}
	if animation != "none" {
		return fmt.Errorf("the lamp is animated while the connection is up: %s", animation)
	}
	return nil
}

// lampIsBreathing asserts a running animation, not a keyframe name, to test
// behaviour rather than CSS.
func (w *world) lampIsBreathing() error {
	animation, err := w.browser.ComputedStyle(".conn-dot", "animation-name")
	if err != nil {
		return err
	}
	if animation == "none" {
		return fmt.Errorf("the offline lamp sits solid")
	}
	cycles, err := w.browser.ComputedStyle(".conn-dot", "animation-iteration-count")
	if err != nil {
		return err
	}
	if cycles != "infinite" {
		return fmt.Errorf("the lamp stops breathing after %s cycles, while the connection stays gone", cycles)
	}
	return nil
}

func (w *world) sidebarOmitsLibrary() error {
	// Wait for the self-rendered sidebar, or absence passes on an empty page.
	if err := w.browser.ExpectVisible("nav-unread"); err != nil {
		return err
	}
	return w.browser.ExpectAbsent("nav-offline")
}

// scriptlessNav is the <noscript> source: with scripting on, its children
// parse as text.
func (w *world) scriptlessNav() (string, error) {
	return harness.Eval[string](w.browser, `document.querySelector('noscript')?.textContent || ''`)
}

func (w *world) scriptlessNavOffersLibrary() error {
	nav, err := w.scriptlessNav()
	if err != nil {
		return err
	}
	if !strings.Contains(nav, `href="/entries/offline"`) {
		return fmt.Errorf("the scriptless navigation has no way to the saved entries: %s", nav)
	}
	return nil
}

func (w *world) scriptlessNavOmitsLibrary() error {
	nav, err := w.scriptlessNav()
	if err != nil {
		return err
	}
	if strings.Contains(nav, "/entries/offline") {
		return fmt.Errorf("the scriptless navigation offers a library nothing is being saved to: %s", nav)
	}
	return nil
}

// openLibraryFromSidebar: offline this navigation is served from cache, so
// offline.js keeps it live.
func (w *world) openLibraryFromSidebar() error {
	if err := w.browser.ClickCSS(`rdrs-sidebar [data-testid="nav-offline"]`); err != nil {
		return err
	}
	return w.expectPath("/entries/offline")
}
