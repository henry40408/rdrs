package views

import (
	"os"
	"regexp"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/flash"
)

const goldenVersion = "@VERSION@"

func readGolden(t *testing.T, name string) string {
	t.Helper()
	data, err := os.ReadFile("testdata/golden/" + name)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

// The shell is compared with the Rust server's own output: everything up to
// the end of the sprite is the same for every page.
func TestDocumentShellMatchesTheRustGolden(t *testing.T) {
	golden := readGolden(t, "login.html")
	end := strings.Index(golden, "</svg>") + len("</svg>")
	got := string(document(documentParts{
		title:      "Login - RDRS",
		gitVersion: goldenVersion,
		head:       moduleScript(goldenVersion, "login.js"),
	}))
	got = strings.TrimSuffix(got, "</body></html>")
	if got != golden[:end] {
		t.Errorf("shell differs from the golden:\n got: %.300s\nwant: %.300s", got[len(got)-300:], golden[end-300:end])
	}
}

func TestEscapingMatchesMaud(t *testing.T) {
	got := string(el("a", attrs(a("href", `x"&<>y`)), text(`<b>"&'`)))
	want := `<a href="x&quot;&amp;&lt;&gt;y">&lt;b&gt;&quot;&amp;'</a>`
	if got != want {
		t.Errorf("got %s, want %s", got, want)
	}
}

func TestSkippedAttributesAndClassHelper(t *testing.T) {
	got := string(el("div", attrs(class("a", "", "b"), when(false, a("id", "x")), when(true, flag("hidden")))))
	if got != `<div class="a b" hidden></div>` {
		t.Errorf("got %s", got)
	}
}

func TestFlashMountRendersEvenWhenEmptyButStaticDoesNot(t *testing.T) {
	if got := string(flashMount(nil)); !strings.Contains(got, `<rdrs-flash class="banner-stack"`) || strings.Contains(got, "banner ") {
		t.Errorf("mount = %s", got)
	}
	if got := flashStatic(nil); got != "" {
		t.Errorf("static = %s", got)
	}
	got := string(flashStatic([]flash.Message{flash.NewError("bad <thing>")}))
	if !strings.Contains(got, `role="alert"`) || !strings.Contains(got, "bad &lt;thing&gt;") {
		t.Errorf("static with message = %s", got)
	}
}

var inlineHandler = regexp.MustCompile(`\son[a-z]+=|\sstyle=|javascript:`)

// The CSP forbids inline script and style, so no view may emit them.
func TestSharedViewsEmitNoInlineScriptOrStyle(t *testing.T) {
	layout := &Layout{GitVersion: "v", SidebarBootstrapJSON: "{}", IsAdmin: true, TotalUnread: 3, OfflineKeep: 2}
	for name, markup := range map[string]node{
		"app page":   appPage(layout, "T", moduleScript("v", "p.js"), flashMount(nil)),
		"row forms":  group(rowMarkerForm(1, true, "t"), rowStarForm(1, false, "t")),
		"pane forms": group(paneStarForm(1, true, "t", true), paneUnreadForm(1, "t", false)),
		"card": group(
			summarizerCardNode(SummarizerCard{State: "completed", Summary: "<p>x</p>"}),
			summarizerCardNode(SummarizerCard{State: "error", Error: "e"}),
			summarizerCardNode(SummarizerCard{State: "queued"})),
	} {
		if inlineHandler.MatchString(string(markup)) {
			t.Errorf("%s emits an inline handler or style", name)
		}
	}
}

func TestAppPageHtmlAttributesAndNavigation(t *testing.T) {
	layout := &Layout{
		Theme: "dark", GitVersion: "v", SidebarBootstrapJSON: `{"a":1}`, IsAdmin: true,
		TotalUnread: 3, OfflineKey: "key", OfflineKeep: 2,
	}
	got := string(appPage(layout, "Feeds", "", ""))
	for _, want := range []string{
		`<html lang="en" data-theme="dark" data-offline-key="key" data-offline-keep="2">`,
		`<title>Feeds - RDRS</title>`,
		`<span class="nav-fallback-count">3</span>`,
		`<a href="/entries/offline">Offline</a>`,
		`<a href="/admin">Admin</a>`,
		`<script type="application/json" id="rdrs-sidebar-bootstrap">{"a":1}</script>`,
		`<rdrs-kb-help data-testid="kb-help"></rdrs-kb-help></body></html>`,
	} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %s", want)
		}
	}
	layout.IsAdmin, layout.TotalUnread, layout.OfflineKeep, layout.Theme = false, 0, 0, ""
	got = string(appPage(layout, "Feeds", "", ""))
	for _, absent := range []string{`/admin`, `nav-fallback-count`, `/entries/offline`, `data-theme`} {
		if strings.Contains(got, absent) {
			t.Errorf("unexpected %s", absent)
		}
	}
}
