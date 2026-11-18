package harness

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/chromedp/cdproto/browser"
	"github.com/chromedp/cdproto/cdp"
	"github.com/chromedp/cdproto/emulation"
	"github.com/chromedp/cdproto/network"
	"github.com/chromedp/cdproto/page"
	"github.com/chromedp/cdproto/target"
	"github.com/chromedp/chromedp"
)

// ScenarioTimeout bounds one scenario, so a wedged Chrome cannot hang the run.
const ScenarioTimeout = 3 * time.Minute

const (
	windowWidth  = 1280
	windowHeight = 720
	closeTimeout = 10 * time.Second
)

// Browser is a headless Chrome of its own, scoped to one scenario.
//
// One process per scenario isolates cookies and storage.
type Browser struct {
	ctx    context.Context
	cancel context.CancelFunc

	mu sync.Mutex
	// opened are the URLs the page asked new windows for, in order.
	opened []string
}

// Scripting says whether the page's own scripts run: the scripted path, or
// the @nojs path.
type Scripting bool

const (
	ScriptsOn  Scripting = true
	ScriptsOff Scripting = false
)

// Open starts a headless Chrome with the page's scripts on.
func Open() (*Browser, error) {
	return OpenWith(ScriptsOn)
}

// OpenWith starts a headless Chrome. Disabled scripting applies from the next
// document on, so it is set before the first navigation.
func OpenWith(scripting Scripting) (*Browser, error) {
	timeoutCtx, cancelTimeout := context.WithTimeout(context.Background(), ScenarioTimeout)
	allocCtx, cancelAlloc := chromedp.NewExecAllocator(timeoutCtx, allocatorOptions()...)
	tabCtx, cancelTab := chromedp.NewContext(allocCtx)
	browser := &Browser{
		ctx: tabCtx,
		cancel: func() {
			cancelTab()
			cancelAlloc()
			cancelTimeout()
		},
	}

	if err := browser.start(scripting); err != nil {
		browser.cancel()
		return nil, err
	}
	return browser, nil
}

// ChromeEnv names the variable that pins the browser binary.
const ChromeEnv = "E2E_CHROME"

// BrowserChoice says which browser Open will start, for the run's log.
func BrowserChoice() string {
	if path := os.Getenv(ChromeEnv); path != "" {
		return ChromeEnv + "=" + path
	}
	return "chromedp's default search (" + ChromeEnv + " is unset)"
}

func allocatorOptions() []chromedp.ExecAllocatorOption {
	// The defaults already carry what the suite needs: headless,
	// no /dev/shm use, and no throttling of a backgrounded renderer (which
	// starves input while CDP script keeps answering, the shape of the clicks
	// CI drops).
	options := append([]chromedp.ExecAllocatorOption{}, chromedp.DefaultExecAllocatorOptions[:]...)
	options = append(options, chromedp.WindowSize(windowWidth, windowHeight),
		// Headless Linux reports `hover: none`, which triggers the touch
		// layout; setEmulatedMedia cannot change it after launch. Values are
		// Blink enums: kHoverHoverType=2, kPointerFine=4.
		chromedp.Flag("blink-settings", "primaryHoverType=2,availableHoverTypes=2,primaryPointerType=4,availablePointerTypes=4"),
		// Classic Linux scrollbars would take 15px off the viewport and break
		// layout assertions on CI only.
		chromedp.Flag("hide-scrollbars", true))
	// chromedp's own search prefers a chromium on PATH over google-chrome, so
	// CI pins the binary it reports.
	if path := os.Getenv(ChromeEnv); path != "" {
		options = append(options, chromedp.ExecPath(path))
	}
	return options
}

func (b *Browser) start(scripting Scripting) error {
	if err := chromedp.Do(b.ctx); err != nil {
		return fmt.Errorf("could not start a browser (%s): a local Chromium or Chrome is required "+
			"(`brew install --cask ungoogled-chromium`), or set %s to its path: %w", BrowserChoice(), ChromeEnv, err)
	}
	if err := chromedp.Listen(b.ctx, b.recordWindowOpen); err != nil {
		return err
	}
	if err := b.SetViewport(windowWidth, windowHeight); err != nil {
		return err
	}
	// The app offers to switch to a browser timezone that differs from the
	// account's (UTC by default); pinning it keeps that banner out of every
	// scenario.
	if err := send(b, emulation.SetTimezoneOverride, emulation.SetTimezoneOverrideParams{TimezoneID: "UTC"}); err != nil {
		return err
	}
	if scripting == ScriptsOff {
		return send(b, emulation.SetScriptExecutionDisabled, emulation.SetScriptExecutionDisabledParams{Value: true})
	}
	return nil
}

// Close ends the browser, and the profile directory goes with it.
func (b *Browser) Close() {
	closing, cancel := context.WithTimeout(b.ctx, closeTimeout)
	defer cancel()
	_ = chromedp.Cancel(closing)
	b.cancel()
}

// EmulateColorScheme emulates prefers-color-scheme with no stored
// preference: the app's system-follow path, which is what the screenshots are
// meant to show.
func (b *Browser) EmulateColorScheme(scheme string) error {
	return send(b, emulation.SetEmulatedMedia, emulation.SetEmulatedMediaParams{
		Media:    "screen",
		Features: []*emulation.MediaFeature{{Name: "prefers-color-scheme", Value: scheme}},
	})
}

// SetViewport sizes the page, and so the screenshots.
func (b *Browser) SetViewport(width, height int64) error {
	return send(b, emulation.SetDeviceMetricsOverride, emulation.SetDeviceMetricsOverrideParams{
		Width:             width,
		Height:            height,
		DeviceScaleFactor: 1,
	})
}

// AddInitScript runs source in every new document before the page's own
// scripts. The page CSP does not apply to it.
func (b *Browser) AddInitScript(source string) error {
	_, err := chromedp.Call(b.ctx, page.AddScriptToEvaluateOnNewDocument, page.AddScriptToEvaluateOnNewDocumentParams{Source: source})
	return err
}

func send[P any](b *Browser, command cdp.Command[P, cdp.Empty], params P) error {
	_, err := chromedp.Call(b.ctx, command, params)
	return err
}

// SetTouch emulates a touch device, the only way to flip (hover: none) and
// (pointer: coarse); setEmulatedMedia ignores both.
func (b *Browser) SetTouch(enabled bool) error {
	return send(b, emulation.SetTouchEmulationEnabled, emulation.SetTouchEmulationEnabledParams{
		Enabled:        enabled,
		MaxTouchPoints: 5,
	})
}

// SetOffline takes the browser offline. Unlike aborting requests, this also
// reaches the service worker.
func (b *Browser) SetOffline(offline bool) error {
	serviceWorkers := true
	_, err := chromedp.Call(b.ctx, network.EmulateNetworkConditionsByRule, network.EmulateNetworkConditionsByRuleParams{
		EmulateOfflineServiceWorker: &serviceWorkers,
		// An empty pattern matches every request; -1 disables throttling, so
		// only the offline flag matters.
		MatchedNetworkConditions: []*network.Conditions{{
			Offline:            offline,
			DownloadThroughput: -1,
			UploadThroughput:   -1,
		}},
	})
	return err
}

// GrantClipboard lets the page write the clipboard; headless Chrome rejects
// clipboard.writeText without it.
func (b *Browser) GrantClipboard() error {
	for _, name := range []string{"clipboard-read", "clipboard-write"} {
		_, err := chromedp.CallBrowser(b.ctx, browser.SetPermission, browser.SetPermissionParams{
			Permission: &browser.PermissionDescriptor{Name: name},
			Setting:    browser.PermissionSettingGranted,
		})
		if err != nil {
			return err
		}
	}
	return nil
}

// Goto navigates to a URL and waits for the page to load.
func (b *Browser) Goto(url string) error {
	return chromedp.Do(b.ctx, chromedp.Navigate(url))
}

// Reload reloads the page and waits for it to load.
func (b *Browser) Reload() error {
	if err := chromedp.Do(b.ctx, chromedp.Reload()); err != nil {
		return err
	}
	return b.WaitForLoad()
}

// WaitForLoad waits for document.readyState to be "complete".
//
// CDP does not wait for a page load after a click, so every read or action
// that follows a navigation waits first. Without it, a key press can run
// before app.js has loaded, and a lookup can run against the old document.
func (b *Browser) WaitForLoad() error {
	return Eventually("the page to finish loading", func() (bool, error) {
		state, err := Eval[string](b, "document.readyState")
		return state == "complete", err
	})
}

// URL is the page's current URL.
func (b *Browser) URL() (*url.URL, error) {
	location, err := chromedp.Run(b.ctx, chromedp.Location())
	if err != nil {
		return nil, err
	}
	return url.Parse(location)
}

// Path is the current URL's path, the shape the steps assert against.
func (b *Browser) Path() (string, error) {
	current, err := b.URL()
	if err != nil {
		return "", err
	}
	return current.Path, nil
}

// PathAndQuery is the current URL's path and query, the shape the steps assert
// against: the port is ephemeral, so full URLs are unstable.
func (b *Browser) PathAndQuery() (string, error) {
	current, err := b.URL()
	if err != nil {
		return "", err
	}
	if current.RawQuery == "" {
		return current.Path, nil
	}
	return current.Path + "?" + current.RawQuery, nil
}

// Screenshot captures the viewport as a PNG.
func (b *Browser) Screenshot() ([]byte, error) {
	return chromedp.Run(b.ctx, chromedp.CaptureScreenshot())
}

// Eval evaluates a JavaScript expression in the page and decodes the result.
//
// Reads go through JavaScript rather than DOM-domain queries, because
// chromedp's queries wait forever by default and the suite's lookups are
// immediate. The page CSP does not apply to it.
func Eval[T any](b *Browser, expression string) (T, error) {
	return chromedp.Run(b.ctx, chromedp.Evaluate[T](expression))
}

// JSLiteral renders a Go value as a JavaScript literal.
func JSLiteral(value any) string {
	encoded, err := json.Marshal(value)
	if err != nil {
		// Only strings, numbers and bools are passed; this cannot happen.
		panic(fmt.Sprintf("encoding %v as a JavaScript literal: %v", value, err))
	}
	return string(encoded)
}

// CookieHeader is the browser's cookie jar as a Cookie header value, plus its
// CSRF token, so a step can act as the signed-in user without the page.
func (b *Browser) CookieHeader() (header, csrf string, err error) {
	result, err := chromedp.Call(b.ctx, network.GetCookies, network.GetCookiesParams{})
	if err != nil {
		return "", "", err
	}
	pairs := make([]string, 0, len(result.Cookies))
	for _, cookie := range result.Cookies {
		pairs = append(pairs, cookie.Name+"="+cookie.Value)
		// The prefixed name wins when a deployment is secure; e2e is plain HTTP.
		if cookie.Name == "__Host-csrf_token" || (cookie.Name == "csrf_token" && csrf == "") {
			csrf = cookie.Value
		}
	}
	return strings.Join(pairs, "; "), csrf, nil
}

// EvalJSON evaluates a JavaScript expression and decodes its JSON result into
// target.
func EvalJSON(b *Browser, expression string, target any) error {
	raw, err := Eval[json.RawMessage](b, expression)
	if err != nil {
		return err
	}
	return json.Unmarshal(raw, target)
}

// EvalAsync is Eval for an expression that yields a promise: it waits for the
// promise and decodes what it resolves to.
func EvalAsync[T any](b *Browser, expression string) (T, error) {
	return chromedp.Run(b.ctx, chromedp.Evaluate[T](expression, chromedp.EvalAwaitPromise))
}

// Back goes one step back in the history and waits for the page to load.
func (b *Browser) Back() error {
	if err := chromedp.Do(b.ctx, chromedp.NavigateBack()); err != nil {
		return err
	}
	return b.WaitForLoad()
}

func (b *Browser) recordWindowOpen(event any) {
	opened, ok := event.(*page.EventWindowOpen)
	if !ok {
		return
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	b.opened = append(b.opened, opened.URL)
}

// WindowOpens counts the new windows the page has asked for so far.
func (b *Browser) WindowOpens() int {
	b.mu.Lock()
	defer b.mu.Unlock()
	return len(b.opened)
}

// WindowOpenURL is the URL of the nth requested window. The requested URL is
// what the app chose; the popup's own URL would collapse to chrome-error://
// where the network is unreachable.
func (b *Browser) WindowOpenURL(index int) string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.opened[index]
}

// ClosePopups closes every tab but the scenario's own.
func (b *Browser) ClosePopups() error {
	own := chromedp.FromContext(b.ctx).Target.TargetID
	result, err := chromedp.CallBrowser(b.ctx, target.GetTargets, target.GetTargetsParams{})
	if err != nil {
		return err
	}
	for _, info := range result.TargetInfos {
		if info.Type != "page" || info.TargetID == own {
			continue
		}
		if _, err := chromedp.CallBrowser(b.ctx, target.CloseTarget, target.CloseTargetParams{TargetID: info.TargetID}); err != nil {
			return err
		}
	}
	return nil
}

// SetTimezone makes the browser report another zone. Pages read it as they
// load, so it applies to pages opened afterwards.
func (b *Browser) SetTimezone(zone string) error {
	return send(b, emulation.SetTimezoneOverride, emulation.SetTimezoneOverrideParams{TimezoneID: zone})
}
