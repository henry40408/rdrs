package sanitize

import (
	"regexp"
	"strings"
	"testing"

	"golang.org/x/net/html"
)

var testSecret = []byte("test_secret_key_32_bytes_long!!!")

func sanitize(input string) string { return HTML(input, Context{Secret: testSecret}) }

func sanitizeWithBase(input, base string) string {
	return HTML(input, Context{Secret: testSecret, BaseURL: base})
}

func mustContain(t *testing.T, output string, wants ...string) {
	t.Helper()
	for _, w := range wants {
		if !strings.Contains(output, w) {
			t.Errorf("missing %q in %s", w, output)
		}
	}
}

func mustNotContain(t *testing.T, output string, unwanted ...string) {
	t.Helper()
	for _, u := range unwanted {
		if strings.Contains(output, u) {
			t.Errorf("unexpected %q in %s", u, output)
		}
	}
}

func TestSummaryKeepsProseAndDropsTheRest(t *testing.T) {
	tests := []struct{ name, in, want string }{
		{"prose", "<p>a <strong>b</strong> <em>c</em></p><ul><li>d</li></ul>", "<p>a <strong>b</strong> <em>c</em></p><ul><li>d</li></ul>"},
		{"script and style with content", "<style>p{}</style><p>x</p><script>alert(1)</script>", "<p>x</p>"},
		{"images and handlers", `<p onclick="x()">hi<img src="https://e/x.png"></p>`, "<p>hi</p>"},
		{"anchors unlinked", `<p>see <a href="https://evil/">here</a></p>`, "<p>see here</p>"},
		{"stray angle bracket", "1 < 2 and 3 > 2", "1 &lt; 2 and 3 &gt; 2"},
		{"markdown stays text", "**bold** and _it_ [l](https://e)", "**bold** and _it_ [l](https://e)"},
	}
	for _, tc := range tests {
		if got := Summary(tc.in); got != tc.want {
			t.Errorf("%s: Summary(%q) = %q, want %q", tc.name, tc.in, got, tc.want)
		}
	}
}

func TestScriptAndJavascriptURLsAreRemoved(t *testing.T) {
	out := sanitize(`<p>Hello</p><script>alert('xss')</script><a href="javascript:alert(1)">Click</a>`)
	mustNotContain(t, out, "script", "alert", "javascript")
	mustContain(t, out, "<p>Hello</p>", "Click")
}

func TestEveryLinkGainsPrivacyAttributes(t *testing.T) {
	out := sanitize(`<a href="https://a.com">A</a><a href="/local">L</a>`)
	mustContain(t, out,
		`<a href="https://a.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">A</a>`,
		`<a href="/local" rel="noopener noreferrer">L</a>`)
}

func TestTrackingImagesAreRemovedButTextStays(t *testing.T) {
	tests := []string{
		`<img src="https://pixel.tracker.com/p.gif" width="1" height="1">`,
		`<img src="https://example.com/a.gif" width="0">`,
		`<img src="https://example.com/a.gif" height="0">`,
		`<img src="https://beacon.example.com/a.gif">`,
		`<img src="https://example.com/x/TRACK/a.gif">`,
		`<img src="https://example.com/1x1.gif">`,
		`<img data-src="https://example.com/real.jpg" width="1" height="1">`,
	}
	for _, img := range tests {
		out := sanitize("<p>keep</p>" + img + "<p>tail</p>")
		if out != "<p>keep</p><p>tail</p>" {
			t.Errorf("%s -> %s", img, out)
		}
	}
}

func TestNormalImageIsNotATrackingPixel(t *testing.T) {
	out := sanitize(`<img src="https://example.com/a.jpg" width="1" height="2">`)
	mustContain(t, out, "/api/proxy/image?url=")
}

func TestTrackingParamsAreStrippedFromLinks(t *testing.T) {
	out := sanitize(`<a href="https://example.com/p?id=1&fbclid=F&utm_source=t&UTM_MEDIUM=m&keep=yes&ref=x">L</a>`)
	mustContain(t, out, `href="https://example.com/p?id=1&keep=yes"`)
	mustNotContain(t, out, "fbclid", "utm_", "&ref=")
}

func TestReadwiseLocationIsStripped(t *testing.T) {
	// The Rust list held the name in mixed case and compared it with a
	// lowercased input, so it never matched.
	if got := StripTrackingParams("https://example.com/a?__readwiseLocation=x&b=1"); got != "https://example.com/a?b=1" {
		t.Fatal(got)
	}
}

func TestStripTrackingParams(t *testing.T) {
	tests := []struct{ in, want string }{
		{"https://example.com/a?utm_source=x&b=1", "https://example.com/a?b=1"},
		{"https://example.com/a?utm_source=x", "https://example.com/a"},
		{"https://example.com/a?clean=1", "https://example.com/a?clean=1"},
		{"https://example.com/a?utm_x=1#frag", "https://example.com/a#frag"},
		{"mailto:a@b.c?utm_source=x", "mailto:a@b.c?utm_source=x"},
		{"/relative?utm_source=x", "/relative?utm_source=x"},
		{"https://Example.com:443/a?mtm_x=1&q=a+b%20c", "https://example.com/a?q=a+b+c"},
		{"https://example.com?fbclid=1", "https://example.com/"},
	}
	for _, tc := range tests {
		if got := StripTrackingParams(tc.in); got != tc.want {
			t.Errorf("StripTrackingParams(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestLazyImagePromotion(t *testing.T) {
	tests := []struct{ name, in string }{
		{"data-lazy-src", `<img src="data:image/gif;base64,R0lGOD" data-lazy-src="https://e.com/real.jpg">`},
		{"data-src without src", `<img data-src="https://e.com/real.jpg">`},
		{"data-original", `<img src="data:image/png;base64,AA" data-original="https://e.com/real.jpg">`},
		{"uppercase attribute", `<img src="data:image/gif;base64,R0lGOD" DATA-SRC="https://e.com/real.jpg">`},
	}
	want := sanitize(`<img src="https://e.com/real.jpg">`)
	for _, tc := range tests {
		if got := sanitize(tc.in); got != want {
			t.Errorf("%s: got %s, want %s", tc.name, got, want)
		}
	}
}

func TestRealSrcIsNotOverriddenByLazyAttribute(t *testing.T) {
	got := sanitize(`<img src="https://e.com/real.jpg" data-src="https://e.com/other.jpg">`)
	if want := sanitize(`<img src="https://e.com/real.jpg">`); got != want {
		t.Fatalf("got %s, want %s", got, want)
	}
}

func TestLazyUrlThatIsAnotherPlaceholderIsIgnored(t *testing.T) {
	out := sanitize(`<img src="data:image/gif;base64,R0lGOD" data-src="data:image/gif;base64,XX" data-lazy-src="https://e.com/real.jpg">`)
	want := sanitize(`<img src="https://e.com/real.jpg">`)
	if out != want {
		t.Fatalf("got %s, want %s", out, want)
	}
}

func TestRelativeImagesResolveAgainstTheBase(t *testing.T) {
	for _, src := range []string{"/images/photo.jpg", "images/photo.jpg", "../images/photo.jpg"} {
		out := sanitizeWithBase(`<p>Text</p><img src="`+src+`" alt="Photo">`, "https://example.com/article/123")
		mustContain(t, out, "/api/proxy/image?url=")
		mustNotContain(t, out, `src="`+src+`"`)
	}
}

func TestRelativeImageWithoutBaseIsDropped(t *testing.T) {
	out := sanitize(`<p>Text</p><img src="/images/photo.jpg" alt="Photo">`)
	mustNotContain(t, out, "<img", "/images/photo.jpg")
	mustContain(t, out, "<p>Text</p>")
}

func TestProtocolRelativeImageNeverEscapesTheProxy(t *testing.T) {
	const input = `<img src="//evil.tld/x.gif">`
	mustNotContain(t, sanitize(input), "evil.tld")
	withBase := sanitizeWithBase(input, "https://feed.example/post")
	mustContain(t, withBase, "/api/proxy/image?url=")
	mustNotContain(t, withBase, `src="//evil.tld`)
}

var srcAttr = regexp.MustCompile(`src="([^"]*)"`)

func TestEveryEmittedImageSrcIsProxiedOrInlineData(t *testing.T) {
	inputs := []string{
		`<img src="//evil.tld/x.gif">`,
		`<img src="/\/evil.tld/x.gif">`,
		`<img src="/images/photo.jpg">`,
		`<img src="images/photo.jpg">`,
		`<img src="../images/photo.jpg">`,
		`<img src="https://cdn.example.com/a.jpg">`,
		`<img src="data:image/svg+xml,%3Csvg%3E%3C/svg%3E" data-src="/img/pic.jpg">`,
		`<img src="">`,
		`<img alt="no src">`,
		`<figure><img src="//evil.tld/a.png"><figcaption>c</figcaption></figure>`,
		`<img src="HTTPS://Evil.tld/a.png">`,
		`<img src="http:///a.png">`,
	}
	for _, base := range []string{"", "https://feed.example/post"} {
		for _, input := range inputs {
			out := HTML(input, Context{Secret: testSecret, BaseURL: base})
			for _, m := range srcAttr.FindAllStringSubmatch(out, -1) {
				if !strings.HasPrefix(m[1], "/api/proxy/image?url=") && !strings.HasPrefix(m[1], "data:") {
					t.Errorf("unproxied src %q escaped from %q (base %q): %s", m[1], input, base, out)
				}
			}
		}
	}
}

func TestProxyBaseAndReferrerArePassedThrough(t *testing.T) {
	out := HTML(`<img src="https://e.com/a.jpg">`, Context{
		Secret: testSecret, Referrer: "https://ref.example/", ProxyBaseURL: "https://rdrs.example/",
	})
	mustContain(t, out, `src="https://rdrs.example/api/proxy/image?url=`, "&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8")
}

func TestAmpersandInImageURLIsNotDoubleEscaped(t *testing.T) {
	out := sanitize(`<img src="https://e.com/a.png?x=1&amp;y=2">`)
	mustContain(t, out, `src="/api/proxy/image?url=`)
	mustNotContain(t, out, "&amp;amp;")
}

func TestGreaterThanInTrackingSrcDoesNotLeakIntoOutput(t *testing.T) {
	out := sanitize(`<p>keep</p><img src="https://pixel.tracker.com/p.gif?q=a>b" width="1" height="1"><p>tail</p>`)
	if out != "<p>keep</p><p>tail</p>" {
		t.Fatal(out)
	}
}

func TestImageDimensionsSurvive(t *testing.T) {
	mustContain(t, sanitize(`<img src="https://e.com/a.jpg" width="640" height="480" alt="x">`),
		`width="640"`, `height="480"`)
}

func TestHarvestDimensions(t *testing.T) {
	tests := []struct{ name, hints, want string }{
		{"data-original", `data-original-width="800" data-original-height="600"`, `width="800" height="600"`},
		{"style", `style="width:320px;height:240px"`, `width="320" height="240"`},
		{"style case and spacing", `style="WIDTH : 10px ; height:20"`, `width="10" height="20"`},
		{"mixed sources", `data-original-width="5" style="height:7px"`, `width="5" height="7"`},
	}
	for _, tc := range tests {
		out := sanitize(`<img src="https://e.com/a.jpg" ` + tc.hints + `>`)
		if !strings.Contains(out, tc.want) {
			t.Errorf("%s: %s", tc.name, out)
		}
	}
}

func TestHarvestSkipsWhenNotApplicable(t *testing.T) {
	tests := []struct{ name, in, unwanted string }{
		{"dimensions present", `<img src="https://e.com/a.jpg" width="100" data-original-width="800" data-original-height="600">`, `800`},
		{"height present", `<img src="https://e.com/a.jpg" height="5" style="width:9px;height:9px">`, `width=`},
		{"zero", `<img src="https://e.com/a.jpg" style="width:0px;height:0px">`, `width=`},
		{"one side only", `<img src="https://e.com/a.jpg" style="width:9px">`, `width=`},
		{"non numeric data attribute", `<img src="https://e.com/a.jpg" data-original-width="8x" data-original-height="6">`, `width=`},
		{"overflow", `<img src="https://e.com/a.jpg" style="width:99999999999;height:5">`, `width=`},
		{"declaration without colon ends the search", `<img src="https://e.com/a.jpg" style="width;width:5px;height:5px">`, `width=`},
	}
	for _, tc := range tests {
		if out := sanitize(tc.in); strings.Contains(out, tc.unwanted) {
			t.Errorf("%s: %s", tc.name, out)
		}
	}
}

func TestImagesAreTaggedAsLoading(t *testing.T) {
	mustContain(t, sanitize(`<img src="https://e.com/a.jpg" alt="x">`),
		`loading="lazy" decoding="async" data-img-state="loading"`)
}

func TestAriaHidden(t *testing.T) {
	gutter := `<div class="language-ts"><pre><code><span class="line">const a = 1;</span>` + "\n" +
		`<span class="line">const b = 2;</span></code></pre>` +
		`<div class="line-numbers-wrapper" aria-hidden="true"><span class="line-number">1</span><br><span class="line-number">2</span><br></div></div>`
	out := sanitize(gutter)
	mustContain(t, out, "const a = 1;", "const b = 2;")
	mustNotContain(t, out, "<br>", "<span>1</span>")

	out = sanitize(`<p aria-hidden="false">keep me</p><p>and me</p>`)
	mustContain(t, out, "keep me", "and me")

	for _, attr := range []string{`aria-hidden="TRUE"`, `ARIA-HIDDEN="true"`} {
		out = sanitize(`<p>body</p><span ` + attr + `>decor</span>`)
		mustContain(t, out, "body")
		mustNotContain(t, out, "decor")
	}
}

func TestWhollyAriaHiddenContentIsKept(t *testing.T) {
	// Blanking the entry is worse than showing markup the author hid.
	mustContain(t, sanitize(`<div aria-hidden="true"><p>the entire article</p></div>`), "the entire article")
}

func TestWhitespaceOnlyDocumentStaysBlank(t *testing.T) {
	if got := sanitize(" \n"); got != " \n" {
		t.Fatalf("%q", got)
	}
}

func TestDocumentWithoutPrePassTriggersIsUnchanged(t *testing.T) {
	const in = "<p>Plain <strong>body</strong> text.</p>"
	if got := sanitize(in); got != in {
		t.Fatal(got)
	}
}

func TestContainsFold(t *testing.T) {
	tests := []struct {
		haystack, needle string
		want             bool
	}{
		{"a <IMG> b", "<img", true},
		{"DATA-Src=", "data-src", true},
		{"xx", "xx", true},
		{"x", "xx", false},
		{"data_src", "data-src", false},
		{"", "", true},
		// strings.EqualFold would fold the Kelvin sign to "k".
		{"K", "k", false},
	}
	for _, tc := range tests {
		if got := containsFold(tc.haystack, tc.needle); got != tc.want {
			t.Errorf("containsFold(%q, %q) = %v", tc.haystack, tc.needle, got)
		}
	}
}

var gateCorpus = []string{
	"",
	"<p>plain</p>",
	`<p>body</p><span ARIA-HIDDEN="true">decor</span>`,
	`<p aria-hidden="false">x</p>`,
	`<IMG SRC="https://e.com/a.jpg">`,
	`<img src="https://e.com/a.jpg" style="width:8px;height:6px">`,
	`<img src="https://e.com/a.jpg" DATA-ORIGINAL-WIDTH="8" DATA-ORIGINAL-HEIGHT="6">`,
	`<image src="https://e.com/a.jpg" data-original-width="8" data-original-height="6">`,
	`<image src="data:image/gif;base64,R0lGOD" data-src="https://e.com/b.jpg">`,
	`<img src="data:image/gif;base64,R0lGOD" DATA-LAZY-SRC="https://e.com/b.jpg">`,
	`<img src="data:image/gif;base64,R0lGOD" data-original="https://e.com/b.jpg">`,
	`<div style="color:red">no image here</div>`,
	`<!-- <img src="x" style="width:8px"> -->`,
	"<p>a &lt;img&gt; mention in text</p>",
}

// Each gate must be a superset of its pass: when it says "skip", running the
// pass anyway must change nothing.
func TestGatesAreSupersetsOfThePassesTheyFront(t *testing.T) {
	passes := []struct {
		name string
		gate func(string) bool
		run  func(*html.Node)
	}{
		{"aria-hidden", ariaHiddenGate, func(n *html.Node) { removeAriaHidden(n) }},
		{"lazy", lazyGate, func(n *html.Node) { eachElement(n, "img", promoteLazyImage) }},
		{"dimensions", dimensionGate, func(n *html.Node) { eachElement(n, "img", harvestDimensions) }},
	}
	for _, p := range passes {
		for _, doc := range gateCorpus {
			if p.gate(doc) {
				continue
			}
			before := render(articlePolicy.clean(parseFragment(doc)))
			root := parseFragment(doc)
			p.run(root)
			if after := render(articlePolicy.clean(root)); after != before {
				t.Errorf("%s gate skipped %q but the pass changes it: %q -> %q", p.name, doc, before, after)
			}
		}
	}
}

func TestSerialisationEscapes(t *testing.T) {
	tests := []struct{ in, want string }{
		{"a &amp; b &lt; c &gt; d", "a &amp; b &lt; c &gt; d"},
		{"nbsp here", "nbsp&nbsp;here"},
		{`<img alt='a"b&c' src="https://e.com/a.png">`, `alt="a&quot;b&amp;c"`},
		{"<br/><br><p/>", "<br><br><p></p>"},
		{"<p>unclosed <b>bold <i>italic", "<p>unclosed <b>bold <i>italic</i></b></p>"},
	}
	for _, tc := range tests {
		if got := sanitize(tc.in); !strings.Contains(got, tc.want) {
			t.Errorf("sanitize(%q) = %q, want it to contain %q", tc.in, got, tc.want)
		}
	}
}

func TestUnknownElementsAreUnwrappedAndDroppedElementsKeepNothing(t *testing.T) {
	tests := []struct{ in, want string }{
		{"<section><p>x</p></section>", "<p>x</p>"},
		{"<script>1</script><style>2</style>k", "k"},
		{"<p>a<!-- hidden -->b</p>", "<p>ab</p>"},
		{"<svg><title>t</title><circle/></svg>after", "after"},
		{"<math><mi>x</mi></math>after", "after"},
		{"<text>quirk</text>after", "after"},
		{"<span>kept</span>", "<span>kept</span>"},
	}
	for _, tc := range tests {
		if got := sanitize(tc.in); got != tc.want {
			t.Errorf("sanitize(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestURLAttributeSchemes(t *testing.T) {
	tests := []struct {
		href    string
		allowed bool
	}{
		{"https://example.com/a", true},
		{"HTTP://EXAMPLE.com", true},
		{"/relative", true},
		{"relative/path", true},
		{"//cdn.example/x", true},
		{"#frag", true},
		{"", true},
		{"javascript:alert(1)", false},
		{" JaVaScRiPt:alert(1)", false},
		{"java\tscript:alert(1)", false},
		{"data:text/html,x", false},
		{"mailto:a@b.c", false},
		{"ftp://x/y", false},
		{"http://", false},
		{"https://exa mple.com/", false},
		{"https://host:99999/", false},
		{"https://[::1]/ok", true},
		{"https://[nope]/", false},
	}
	for _, tc := range tests {
		if got := urlAttrAllowed(tc.href); got != tc.allowed {
			t.Errorf("urlAttrAllowed(%q) = %v, want %v", tc.href, got, tc.allowed)
		}
	}
}

func TestNamespaceQuirks(t *testing.T) {
	// ammonia removes MathML and SVG children once their wrapper is unwrapped.
	if got := Summary("<math><mi>x</mi></math>y"); got != "y" {
		t.Fatal(got)
	}
	// <svg> is not an allowed tag, so its descendants have no legal parent.
	if got := sanitize("<svg><foreignObject><p>in</p></foreignObject></svg>"); got != "" {
		t.Fatal(got)
	}
}

func TestDuplicateAttributesKeepTheFirst(t *testing.T) {
	out := sanitize(`<img src="https://e.com/a.jpg" alt="first" alt="second">`)
	mustContain(t, out, `alt="first"`)
	mustNotContain(t, out, "second")
}

func TestLinkWithoutHrefStillGetsRel(t *testing.T) {
	if got := sanitize(`<a name="x">t</a>`); got != `<a rel="noopener noreferrer">t</a>` {
		t.Fatal(got)
	}
}

func TestMalformedHrefQueryIsLeftAlone(t *testing.T) {
	href := "https://example.com/a?x=%zz&utm_source=1"
	if got := StripTrackingParams(href); got != "https://example.com/a?x=%25zz" {
		t.Fatal(got)
	}
}
