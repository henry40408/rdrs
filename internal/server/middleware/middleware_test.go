package middleware

import (
	"bytes"
	"compress/gzip"
	"context"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/andybalholm/brotli"
)

func serve(handler http.Handler, method, target string, headers ...string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(method, target, nil)
	for i := 0; i+1 < len(headers); i += 2 {
		req.Header.Set(headers[i], headers[i+1])
	}
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, req)
	return rec
}

func respond(status int, contentType, body string) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		if contentType != "" {
			w.Header().Set("Content-Type", contentType)
		}
		w.WriteHeader(status)
		_, _ = w.Write([]byte(body))
	})
}

func TestChainRunsTheFirstLayerOutermost(t *testing.T) {
	var order []string
	layer := func(name string) Middleware {
		return func(next http.Handler) http.Handler {
			return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				order = append(order, name+" in")
				next.ServeHTTP(w, r)
				order = append(order, name+" out")
			})
		}
	}
	serve(Chain(respond(200, "", ""), layer("a"), layer("b")), "GET", "/")
	if got := strings.Join(order, ","); got != "a in,b in,b out,a out" {
		t.Errorf("order = %s", got)
	}
}

func TestSecurityHeadersKeepExistingValues(t *testing.T) {
	handler := SecurityHeaders(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("X-Frame-Options", "SAMEORIGIN")
	}))
	rec := serve(handler, "GET", "/") // writes nothing: the head still gets the headers
	want := map[string]string{
		"Content-Security-Policy":    contentSecurityPolicy,
		"X-Content-Type-Options":     "nosniff",
		"Referrer-Policy":            "strict-origin-when-cross-origin",
		"Permissions-Policy":         permissionsPolicy,
		"X-Frame-Options":            "SAMEORIGIN",
		"Cross-Origin-Opener-Policy": "same-origin",
	}
	for name, value := range want {
		if got := rec.Header().Get(name); got != value {
			t.Errorf("%s = %q, want %q", name, got, value)
		}
	}
	if rec.Header().Get("Cross-Origin-Resource-Policy") != "" {
		t.Error("CORP must stay absent")
	}
	if strings.Contains(permissionsPolicy, "publickey-credentials") {
		t.Error("naming publickey-credentials would break passkeys")
	}
}

func TestHSTSKeepsAProxyHeader(t *testing.T) {
	value := "max-age=31536000; includeSubDomains"
	rec := serve(HSTS(value)(respond(200, "", "x")), "GET", "/")
	if got := rec.Header().Get("Strict-Transport-Security"); got != value {
		t.Errorf("got %q", got)
	}
	existing := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Strict-Transport-Security", "max-age=1")
	})
	if got := serve(HSTS(value)(existing), "GET", "/").Header().Get("Strict-Transport-Security"); got != "max-age=1" {
		t.Errorf("overwrote the proxy header: %q", got)
	}
}

func TestDateHeaderReplacesAndUsesGMT(t *testing.T) {
	handler := DateHeader(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Date", "stale")
	}))
	got := serve(handler, "GET", "/").Header().Get("Date")
	parsed, err := http.ParseTime(got)
	if err != nil || time.Since(parsed) > time.Minute || !strings.HasSuffix(got, " GMT") {
		t.Errorf("Date = %q, %v", got, err)
	}
}

type logRecord struct {
	level slog.Level
	attrs map[string]any
}

type captureHandler struct {
	mu      *sync.Mutex
	records *[]logRecord
}

func (h captureHandler) Enabled(context.Context, slog.Level) bool { return true }
func (h captureHandler) Handle(_ context.Context, r slog.Record) error {
	attrs := map[string]any{}
	r.Attrs(func(a slog.Attr) bool { attrs[a.Key] = a.Value.Any(); return true })
	h.mu.Lock()
	defer h.mu.Unlock()
	*h.records = append(*h.records, logRecord{r.Level, attrs})
	return nil
}
func (h captureHandler) WithAttrs([]slog.Attr) slog.Handler { return h }
func (h captureHandler) WithGroup(string) slog.Handler      { return h }

func captureLogs(t *testing.T) *[]logRecord {
	t.Helper()
	previous := slog.Default()
	records := &[]logRecord{}
	slog.SetDefault(slog.New(captureHandler{mu: &sync.Mutex{}, records: records}))
	t.Cleanup(func() { slog.SetDefault(previous) })
	return records
}

func TestRequestLogLabelsTheRouteTemplateNotThePath(t *testing.T) {
	records := captureLogs(t)
	routed := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		SetRoute(r, "/invite/{token}")
		w.WriteHeader(204)
	})
	serve(RequestLog(routed), "GET", "/invite/secret-token")
	serve(RequestLog(respond(404, "", "")), "GET", "/secret/path")
	if len(*records) != 2 {
		t.Fatalf("%d records", len(*records))
	}
	first, second := (*records)[0], (*records)[1]
	if first.level != slog.LevelDebug || first.attrs["event"] != "http.request" ||
		first.attrs["route"] != "/invite/{token}" || first.attrs["status"] != int64(204) && first.attrs["status"] != 204 {
		t.Errorf("first = %+v", first)
	}
	if second.attrs["route"] != UnmatchedRoute {
		t.Errorf("second route = %v", second.attrs["route"])
	}
	for _, record := range *records {
		for _, value := range record.attrs {
			if s, ok := value.(string); ok && strings.Contains(s, "secret") {
				t.Errorf("a path leaked into the log: %v", record.attrs)
			}
		}
	}
}

func TestRequestLogIsSlowAtExactlyTheThreshold(t *testing.T) {
	records := captureLogs(t)
	logCompleted(completedRequest{method: "GET", route: "/x", status: 200, elapsed: SlowRequestThreshold - 1})
	logCompleted(completedRequest{method: "GET", route: "/x", status: 200, elapsed: SlowRequestThreshold})
	got := *records
	if got[0].level != slog.LevelDebug || got[0].attrs["event"] != "http.request" {
		t.Errorf("below threshold: %+v", got[0])
	}
	if got[1].level != slog.LevelWarn || got[1].attrs["event"] != "http.slow_request" {
		t.Errorf("at threshold: %+v", got[1])
	}
}

func TestCrossSiteGuardMatrix(t *testing.T) {
	cases := []struct {
		name    string
		method  string
		headers []string
		want    int
	}{
		{"get is safe", "GET", []string{"Sec-Fetch-Site", "cross-site"}, 200},
		{"head is safe", "HEAD", []string{"Sec-Fetch-Site", "cross-site"}, 200},
		{"options is safe", "OPTIONS", []string{"Sec-Fetch-Site", "cross-site"}, 200},
		{"trace is not exempt", "TRACE", []string{"Sec-Fetch-Site", "cross-site"}, 403},
		{"same-origin", "POST", []string{"Sec-Fetch-Site", "same-origin"}, 200},
		{"none", "POST", []string{"Sec-Fetch-Site", "none"}, 200},
		{"cross-site", "POST", []string{"Sec-Fetch-Site", "cross-site"}, 403},
		{"same-site", "POST", []string{"Sec-Fetch-Site", "same-site"}, 403},
		{"no headers", "POST", nil, 200},
		{"origin matching host", "POST", []string{"Origin", "https://example.com"}, 200},
		{"origin with path", "POST", []string{"Origin", "https://example.com/x"}, 200},
		{"origin other host", "POST", []string{"Origin", "https://evil.example"}, 403},
		{"origin case differs", "POST", []string{"Origin", "https://EXAMPLE.com"}, 403},
		{"origin has other port", "POST", []string{"Origin", "https://example.com:8443"}, 403},
		{"origin has userinfo", "POST", []string{"Origin", "https://u@example.com"}, 403},
		{"null origin", "POST", []string{"Origin", "null"}, 403},
		{"non-http origin", "POST", []string{"Origin", "ftp://example.com"}, 403},
		{"sec-fetch-site wins over origin", "POST", []string{"Sec-Fetch-Site", "same-origin", "Origin", "https://evil.example"}, 200},
	}
	for _, c := range cases {
		req := httptest.NewRequest(c.method, "http://example.com/x", nil)
		req.Host = "example.com"
		for i := 0; i+1 < len(c.headers); i += 2 {
			req.Header.Set(c.headers[i], c.headers[i+1])
		}
		rec := httptest.NewRecorder()
		CrossSiteGuard(respond(200, "", "ok")).ServeHTTP(rec, req)
		if rec.Code != c.want {
			t.Errorf("%s: status %d, want %d", c.name, rec.Code, c.want)
		}
		if c.want == 403 && rec.Body.Len() != 0 {
			t.Errorf("%s: refusal body %q, want empty", c.name, rec.Body)
		}
	}
}

func TestCrossSiteGuardLogsTheCheckThatFired(t *testing.T) {
	records := captureLogs(t)
	serve(CrossSiteGuard(respond(200, "", "")), "POST", "/x", "Sec-Fetch-Site", "cross-site")
	serve(CrossSiteGuard(respond(200, "", "")), "POST", "/x", "Origin", "https://evil.example")
	got := *records
	if len(got) != 2 || got[0].attrs["check"] != "sec_fetch_site" || got[1].attrs["check"] != "origin_fallback" ||
		got[0].attrs["event"] != "csrf.cross_site" || got[1].attrs["sec_fetch_site"] != "-" {
		t.Errorf("records = %+v", got)
	}
}

func TestTimeoutAnswers408BeforeTheHead(t *testing.T) {
	stuck := http.HandlerFunc(func(_ http.ResponseWriter, r *http.Request) { <-r.Context().Done() })
	rec := serve(Timeout(20*time.Millisecond)(stuck), "GET", "/")
	if rec.Code != http.StatusRequestTimeout || rec.Body.Len() != 0 {
		t.Errorf("got %d %q", rec.Code, rec.Body)
	}
}

func TestTimeoutDropsALateWrite(t *testing.T) {
	release := make(chan struct{})
	finished := make(chan struct{})
	slow := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		defer close(finished)
		<-release
		w.Header().Set("X-Late", "1")
		w.WriteHeader(200)
		_, _ = w.Write([]byte("late"))
	})
	rec := serve(Timeout(10*time.Millisecond)(slow), "GET", "/")
	close(release)
	<-finished
	if rec.Code != 408 || rec.Body.Len() != 0 || rec.Header().Get("X-Late") != "" {
		t.Errorf("got %d %q %v", rec.Code, rec.Body, rec.Header())
	}
}

func TestTimeoutDoesNotCutAResponseAlreadyStarted(t *testing.T) {
	streaming := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte("a"))
		time.Sleep(60 * time.Millisecond)
		_, _ = w.Write([]byte("b"))
	})
	rec := serve(Timeout(20*time.Millisecond)(streaming), "GET", "/")
	if rec.Code != 200 || rec.Body.String() != "ab" {
		t.Errorf("got %d %q", rec.Code, rec.Body)
	}
}

func TestTimeoutPassesAFastResponseAndPropagatesPanics(t *testing.T) {
	rec := serve(Timeout(time.Second)(respond(201, "text/plain", "hi")), "GET", "/")
	if rec.Code != 201 || rec.Body.String() != "hi" {
		t.Errorf("got %d %q", rec.Code, rec.Body)
	}
	defer func() {
		if recover() == nil {
			t.Error("a handler panic was swallowed")
		}
	}()
	serve(Timeout(time.Second)(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { panic("boom") })), "GET", "/")
}

func TestETagTagsHTMLAndAnswers304(t *testing.T) {
	handler := ETag(respond(200, "text/html; charset=utf-8", "<p>hi</p>"))
	first := serve(handler, "GET", "/")
	etag := first.Header().Get("ETag")
	if !strings.HasPrefix(etag, `W/"`) || len(etag) != len(`W/""`)+16 || first.Body.String() != "<p>hi</p>" {
		t.Fatalf("etag %q body %q", etag, first.Body)
	}
	for _, match := range []string{etag, strings.TrimPrefix(etag, "W/"), `"other", ` + etag, "  " + etag} {
		rec := serve(handler, "GET", "/", "If-None-Match", match)
		if rec.Code != 304 || rec.Body.Len() != 0 || rec.Header().Get("Content-Type") != "" {
			t.Errorf("If-None-Match %q: %d %q %v", match, rec.Code, rec.Body, rec.Header())
		}
	}
	if rec := serve(handler, "GET", "/", "If-None-Match", `W/"nope"`); rec.Code != 200 {
		t.Errorf("a different tag gave %d", rec.Code)
	}
}

func TestETagLeavesOtherResponsesAlone(t *testing.T) {
	for name, handler := range map[string]http.Handler{
		"json":     respond(200, "application/json", "{}"),
		"error":    respond(404, "text/html", "<p>no</p>"),
		"redirect": respond(302, "text/html", ""),
		"no type":  respond(200, "", "x"),
	} {
		if rec := serve(ETag(handler), "GET", "/"); rec.Header().Get("ETag") != "" {
			t.Errorf("%s was tagged", name)
		}
	}
}

func TestETagRefusesAnOversizedBody(t *testing.T) {
	big := strings.Repeat("a", maxETagBody+1)
	if rec := serve(ETag(respond(200, "text/html", big)), "GET", "/"); rec.Code != 500 {
		t.Errorf("status %d, want 500", rec.Code)
	}
	exact := strings.Repeat("a", maxETagBody)
	if rec := serve(ETag(respond(200, "text/html", exact)), "GET", "/"); rec.Code != 200 {
		t.Errorf("a body exactly at the limit gave %d", rec.Code)
	}
}

func gunzip(t *testing.T, data []byte) string {
	t.Helper()
	reader, err := gzip.NewReader(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	out, err := io.ReadAll(reader)
	if err != nil {
		t.Fatal(err)
	}
	return string(out)
}

func TestCompressEncodesWithGzipAndBrotli(t *testing.T) {
	body := strings.Repeat("hello world ", 20)
	handler := Compress(respond(200, "text/html", body))
	rec := serve(handler, "GET", "/", "Accept-Encoding", "gzip")
	if rec.Header().Get("Content-Encoding") != "gzip" || gunzip(t, rec.Body.Bytes()) != body {
		t.Errorf("gzip: %v", rec.Header())
	}
	if rec.Header().Get("Vary") != "Accept-Encoding" || rec.Header().Get("Content-Length") != "" {
		t.Errorf("headers %v", rec.Header())
	}
	rec = serve(handler, "GET", "/", "Accept-Encoding", "gzip, br")
	decoded, err := io.ReadAll(brotli.NewReader(rec.Body))
	if rec.Header().Get("Content-Encoding") != "br" || err != nil || string(decoded) != body {
		t.Errorf("br: %v %v", rec.Header(), err)
	}
}

func TestCompressSkipsWhatTowerHttpSkips(t *testing.T) {
	long := strings.Repeat("x", 100)
	cases := map[string]http.Handler{
		"image": respond(200, "image/png", long),
		"sse":   respond(200, "text/event-stream", long),
		"grpc":  respond(200, "application/grpc+proto", long),
		"small": respond(200, "text/html", strings.Repeat("x", minCompressSize-1)),
		"encoded": http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Encoding", "x")
			_, _ = w.Write([]byte(long))
		}),
		"ranged": http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Range", "bytes 0-1/2")
			_, _ = w.Write([]byte(long))
		}),
	}
	for name, handler := range cases {
		rec := serve(Compress(handler), "GET", "/", "Accept-Encoding", "gzip")
		if rec.Header().Get("Content-Encoding") == "gzip" {
			t.Errorf("%s was compressed", name)
		}
	}
	for name, handler := range map[string]http.Handler{
		"svg":      respond(200, "image/svg+xml", long),
		"grpc-web": respond(200, "application/grpc-web", long),
		"at limit": respond(200, "text/html", strings.Repeat("x", minCompressSize)),
	} {
		if rec := serve(Compress(handler), "GET", "/", "Accept-Encoding", "gzip"); rec.Header().Get("Content-Encoding") != "gzip" {
			t.Errorf("%s was not compressed", name)
		}
	}
}

func TestChooseEncoding(t *testing.T) {
	cases := []struct {
		header     []string
		encoding   string
		acceptable bool
	}{
		{nil, "", true},
		{[]string{"gzip"}, "gzip", true},
		{[]string{"x-gzip"}, "gzip", true},
		{[]string{"GZIP"}, "gzip", true},
		{[]string{"br"}, "br", true},
		{[]string{"gzip, br"}, "br", true},
		{[]string{"br, gzip"}, "br", true},
		{[]string{"gzip;q=1, br;q=0.5"}, "gzip", true},
		{[]string{"gzip;q=0, br"}, "br", true},
		{[]string{"gzip;q=0"}, "", true},
		{[]string{"identity"}, "", true},
		{[]string{"identity, gzip"}, "gzip", true},
		{[]string{"deflate, zstd"}, "", true},
		{[]string{"identity;q=0"}, "", false},
		{[]string{"identity;q=0, gzip"}, "gzip", true},
		{[]string{"*"}, "br", true},
		{[]string{"*;q=0"}, "", false},
		{[]string{"gzip, *;q=0"}, "gzip", true},
		{[]string{"*;q=0.1, gzip;q=0.5"}, "gzip", true},
		{[]string{"gzip;q=0.5, br;q=0.5"}, "br", true},
		{[]string{"gzip;q=abc, br"}, "br", true},
		{[]string{"gzip;q=1.5"}, "", true},
		{[]string{"gzip;q=0.1234"}, "", true},
		{[]string{"gzip", "br;q=0.1"}, "gzip", true},
	}
	for _, c := range cases {
		encoding, acceptable := chooseEncoding(c.header)
		if encoding != c.encoding || acceptable != c.acceptable {
			t.Errorf("%q: got (%q, %v), want (%q, %v)", c.header, encoding, acceptable, c.encoding, c.acceptable)
		}
	}
}

func TestCompressRefusalIs406WithVary(t *testing.T) {
	rec := serve(Compress(respond(200, "text/html", "page")), "GET", "/", "Accept-Encoding", "identity;q=0")
	if rec.Code != http.StatusNotAcceptable || rec.Header().Get("Vary") != "Accept-Encoding" || rec.Body.String() != "page" {
		t.Errorf("got %d %v %q", rec.Code, rec.Header(), rec.Body)
	}
	implicit := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("page")) })
	if rec := serve(Compress(implicit), "GET", "/", "Accept-Encoding", "identity;q=0"); rec.Code != 406 {
		t.Errorf("implicit 200 gave %d", rec.Code)
	}
}

func TestParseQValue(t *testing.T) {
	good := map[string]int{"q=0": 0, "q=1": 1000, "Q=0.5": 500, "q=0.001": 1, "q=1.000": 1000, "q=0.": 0}
	for text, want := range good {
		if got, ok := parseQValue(text); !ok || got != want {
			t.Errorf("%q: got %d %v, want %d", text, got, ok, want)
		}
	}
	for _, text := range []string{"", "q", "q=", "q=2", "q=1.1", "q=0.1234", "x=1", "q=a", "q=0,5"} {
		if _, ok := parseQValue(text); ok {
			t.Errorf("%q was accepted", text)
		}
	}
}

func TestLayersCompose(t *testing.T) {
	body := "<p>" + strings.Repeat("hello ", 30) + "</p>"
	handler := Chain(respond(200, "text/html", body), Compress, DateHeader, ETag)
	rec := serve(handler, "GET", "/", "Accept-Encoding", "gzip")
	if rec.Header().Get("ETag") == "" || rec.Header().Get("Date") == "" || gunzip(t, rec.Body.Bytes()) != body {
		t.Errorf("headers %v", rec.Header())
	}
	// The tag is of the uncompressed body, so a client can revalidate whatever
	// encoding it negotiates.
	if rec.Header().Get("ETag") != weakETag([]byte(body)) {
		t.Error("ETag was computed over the compressed body")
	}
}

func TestTimeoutHandlerMayEditHeadersAfterExpiry(t *testing.T) {
	// Run under -race: the handler holds w.Header() across the expiry while the
	// outer layers edit the real map.
	handler := Chain(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		h := w.Header()
		time.Sleep(60 * time.Millisecond)
		h.Set("X-Late", "1")
		w.WriteHeader(http.StatusOK)
	}), SecurityHeaders, Timeout(20*time.Millisecond))
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/", nil))
	time.Sleep(100 * time.Millisecond)
	if rec.Code != http.StatusRequestTimeout || rec.Header().Get("X-Late") != "" {
		t.Errorf("status %d, headers %v", rec.Code, rec.Header())
	}
}

func TestTimeoutRepanicsWithTheOriginalValue(t *testing.T) {
	handler := Timeout(time.Second)(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
		panic(http.ErrAbortHandler)
	}))
	defer func() {
		if got := recover(); got != http.ErrAbortHandler {
			t.Errorf("recovered %v", got)
		}
	}()
	handler.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(http.MethodGet, "/", nil))
}
