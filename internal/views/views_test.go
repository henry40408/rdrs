package views

import (
	"crypto/sha256"
	"encoding/hex"
	"os"
	"regexp"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/flash"
)

const goldenVersion = "@VERSION@"

// goldenDigests are the sha256 of the pages the Rust server rendered, by the
// name of the golden file under internal/views/testdata/golden. The files are
// deleted; git history holds them. Never recompute a digest to make a test
// pass: a difference is a behaviour change.
var goldenDigests = map[string]string{
	"login.html":                 "afa21100232d041be86dcb26aef5862c91af6b7247c64a5f5ec100f8684340bf",
	"login_next.html":            "3f3dd1aab65e9d265d1c55948ccdcaf8361d92fec4a90d0a49ef0ecb85fe0742",
	"login_bad_credentials.html": "64df6c77537edd204e2610fad69ee47beed7970ae699a0935598aa65e4a3caec",
	"login_setup_available.html": "0b34404050da6ac487535129cfab20a4c2528041a28f20f53782de2e758c23f7",
	"setup.html":                 "61676b8e1885c452cf08fdecc819ba9c013ad0ebfac93c0356bbe39c46ba7673",
	"setup_mismatch.html":        "5be1b09165d460b02bbeb99688bd2688be8a385121ca5ceaf898a4ac26e41953",
	"setup_bad_token.html":       "7ca26f9d8682aa1a056132b4ab5064e745cd5337d95fbe004467c19ab76b4e46",
	"invite.html":                "d7cb6eeebf3e37d668ad2180ec9cefd15627a4b9ed5437b48f2a36f73501ba30",
	"invite_mismatch.html":       "40c60c296e4ffbf6267e0a31aa38baaffb53ebfdd2beb03facf29586c05579e3",
	"invite_invalid.html":        "7bfe03397284b3c8a09921e47ff143f4064294a0dcf2fd9268a2e2882aed645c",
}

// loginShellDigest is the digest of login.html up to the end of the sprite.
const loginShellDigest = "c906a1c9c3c76eb4249da7890fab40bad09c877669eb85b69a0a01b8e71da47d"

// assertPageGolden compares the digest of got with want, the digest of the
// Rust server's golden file. A mismatch writes got to a temporary file; the
// golden is in git history.
func assertPageGolden(t *testing.T, file, want, got string) {
	t.Helper()
	sum := sha256.Sum256([]byte(got))
	if hex.EncodeToString(sum[:]) == want {
		return
	}
	actual, err := os.CreateTemp("", file+".actual.*")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = actual.Close() }()
	if _, err := actual.WriteString(got); err != nil {
		t.Fatal(err)
	}
	path := "internal/views/testdata/golden/" + file
	t.Errorf("%s differs from the Rust golden (%d bytes of output written to %s).\n"+
		"The golden is in git history: git show $(git log --diff-filter=D --format=%%h -1 -- %s)^:%s",
		file, len(got), actual.Name(), path, path)
}

// The shell is compared with the Rust server's own output: everything up to
// the end of the sprite is the same for every page. The digest covers only that
// part of login.html, so the recovered golden is longer than the output.
func TestDocumentShellMatchesTheRustGolden(t *testing.T) {
	got := string(document(documentParts{
		title:      "Login - RDRS",
		gitVersion: goldenVersion,
		head:       moduleScript(goldenVersion, "login.js"),
	}))
	got = strings.TrimSuffix(got, "</body></html>")
	assertPageGolden(t, "login.html", loginShellDigest, got)
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
