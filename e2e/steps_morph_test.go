package e2e

import (
	"fmt"
	"time"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// In-place-swap assertions: tag the DOM, act, check the tags survived.
//
// Tags are JS properties, not data- attributes: an attribute changes
// outerHTML, which performSwap compares to skip unchanged rows. (The sidebar
// reconciles instead, so it uses attributes.)

// renderStampAge outlasts data-snapshot-at's one-second resolution.
const renderStampAge = 1100 * time.Millisecond

// taggedTally is how many of a set of nodes still carry a tag.
type taggedTally struct {
	Container bool `json:"container"`
	Total     int  `json:"total"`
	Tagged    int  `json:"tagged"`
}

func registerMorphSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I tag the entry rows$`, w.tagEntryRows)
	sc.Step(`^the entry rows are still the ones I tagged$`, w.entryRowsStillTagged)
	sc.Step(`^I tag the entry list contents$`, w.tagListContents)
	sc.Step(`^the entry list contents are still the ones I tagged$`, func() error {
		return w.expectStillTagged(stillTaggedCheck{
			variable: "list", selector: `[data-entries-list]`, tag: "__e2eMorphTag", what: "list container",
			nodes: `list.querySelectorAll("[data-entry-row]"), ...list.querySelectorAll("img.entry-favicon")`,
		})
	})
	// Arms the check for the next list-pane render, before the click.
	sc.Step(`^I tag the entry list pane$`, w.tagListPane)
	sc.Step(`^the entry list pane is still the one I tagged$`, w.paneStillTagged)
	// data-snapshot-at has one-second resolution.
	sc.Step(`^I let the render stamp age$`, func() error { time.Sleep(renderStampAge); return nil })
	sc.Step(`^the list's render stamp has advanced$`, w.stampAdvanced)
	// A document load wipes window, so a surviving marker proves no reload
	// (which would reset the sidebar scroll).
	sc.Step(`^I mark the document for reload detection$`, func() error {
		_, err := harness.Eval[bool](w.browser, `(window.__rdrsDocumentMarker = true)`)
		return err
	})
	sc.Step(`^the document did not reload$`, func() error {
		marker, err := harness.Eval[bool](w.browser, `window.__rdrsDocumentMarker === true`)
		if err == nil && !marker {
			err = fmt.Errorf("the marker is gone, so the document reloaded")
		}
		return err
	})
	sc.Step(`^the entry list favicons have loaded$`, func() error {
		if _, err := w.browser.CSS("[data-entries-list] img.entry-favicon"); err != nil {
			return err
		}
		return harness.Eventually("every list favicon to finish loading", func() (bool, error) {
			return harness.Eval[bool](w.browser, `(() => {
				const icons = [...document.querySelectorAll("[data-entries-list] img.entry-favicon")];
				return icons.length > 0 && icons.every((i) => i.complete && i.naturalWidth > 0);
			})()`)
		})
	})
}

func (w *world) tagEntryRows() error {
	tagged, err := harness.Eval[int](w.browser, `(() => {
		const nodes = document.querySelectorAll("[data-entry-row], [data-entry-row] *");
		for (const node of nodes) node.__e2eTag = true;
		return nodes.length;
	})()`)
	if err == nil && tagged == 0 {
		err = fmt.Errorf("there were no entry rows to tag")
	}
	return err
}

func (w *world) entryRowsStillTagged() error {
	rebuilt, err := harness.Eval[[]string](w.browser, `[...document.querySelectorAll("[data-entry-row], [data-entry-row] *")]
		.filter((node) => !node.__e2eTag)
		.map((node) => node.nodeName.toLowerCase() + "." + node.className + " in #" +
			node.closest("[data-entry-row]")?.id)`)
	if err == nil && len(rebuilt) > 0 {
		err = fmt.Errorf("these nodes were rebuilt rather than left alone: %q", rebuilt)
	}
	return err
}

func (w *world) tagListContents() error {
	var counts struct{ Rows, Icons int }
	err := harness.EvalJSON(w.browser, `(() => {
		const list = document.querySelector("[data-entries-list]");
		const rows = [...list.querySelectorAll("[data-entry-row]")];
		const icons = [...list.querySelectorAll("img.entry-favicon")];
		list.__e2eMorphTag = true;
		for (const node of [...rows, ...icons]) node.__e2eMorphTag = true;
		return { rows: rows.length, icons: icons.length };
	})()`, &counts)
	if err != nil {
		return err
	}
	if counts.Rows == 0 {
		return fmt.Errorf("the list had no rows to tag")
	}
	if counts.Icons == 0 {
		return fmt.Errorf("the list had no favicons to tag")
	}
	return nil
}

func (w *world) tagListPane() error {
	var state struct {
		Rows  int
		Icons int
		Stamp *string
	}
	err := harness.EvalJSON(w.browser, `(() => {
		const pane = document.querySelector("[data-list-pane]");
		pane.__e2ePaneTag = true;
		const rows = [...pane.querySelectorAll("[data-entry-row]")];
		const icons = [...pane.querySelectorAll("img")];
		for (const node of [...rows, ...icons]) node.__e2ePaneTag = true;
		return { rows: rows.length, icons: icons.length,
			stamp: pane.querySelector("[data-snapshot-at]")?.getAttribute("data-snapshot-at") ?? null };
	})()`, &state)
	if err != nil {
		return err
	}
	if state.Rows == 0 {
		return fmt.Errorf("the pane had no rows to tag")
	}
	if state.Icons == 0 {
		return fmt.Errorf("the pane had no images to tag")
	}
	if state.Stamp != nil {
		w.paneStamp = *state.Stamp
	}
	return nil
}

func (w *world) paneStillTagged() error {
	// Two frames for the swap logic to run.
	if _, err := harness.EvalAsync[bool](w.browser,
		`new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true))))`); err != nil {
		return err
	}
	return w.expectStillTagged(stillTaggedCheck{
		variable: "pane", selector: `[data-list-pane]`, tag: "__e2ePaneTag", what: "list pane",
		nodes: `pane.querySelectorAll("[data-entry-row]"), ...pane.querySelectorAll("img")`,
	})
}

// stillTaggedCheck describes one container and the nodes inside it that must
// keep their tag property.
type stillTaggedCheck struct {
	// variable names the container inside the script, which nodes refers to.
	variable string
	selector string
	// nodes is a comma-separated list of NodeLists to spread.
	nodes string
	tag   string
	what  string
}

func (w *world) expectStillTagged(check stillTaggedCheck) error {
	var kept taggedTally
	err := harness.EvalJSON(w.browser, fmt.Sprintf(`(() => {
		const %[1]s = document.querySelector(%[2]s);
		const nodes = [...%[3]s];
		return { container: %[1]s.%[4]s === true, total: nodes.length,
			tagged: nodes.filter((n) => n.%[4]s).length };
	})()`, check.variable, harness.JSLiteral(check.selector), check.nodes, check.tag), &kept)
	if err != nil {
		return err
	}
	if !kept.Container {
		return fmt.Errorf("the %s itself was replaced", check.what)
	}
	if kept.Total == 0 {
		return fmt.Errorf("the %s is empty after the interaction", check.what)
	}
	if kept.Tagged != kept.Total {
		return fmt.Errorf("%d of %d nodes were rebuilt", kept.Total-kept.Tagged, kept.Total)
	}
	return nil
}

func (w *world) stampAdvanced() error {
	if w.paneStamp == "" {
		return fmt.Errorf("no render stamp was captured")
	}
	return harness.Eventually("the render stamp to advance", func() (bool, error) {
		stamp, err := harness.Eval[*string](w.browser,
			`document.querySelector("[data-list-pane] [data-snapshot-at]")?.getAttribute("data-snapshot-at") ?? null`)
		return stamp == nil || *stamp != w.paneStamp, err
	})
}
