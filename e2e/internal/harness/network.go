package harness

import (
	"fmt"
	"regexp"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/chromedp/cdproto/fetch"
	"github.com/chromedp/cdproto/network"
	"github.com/chromedp/chromedp"
)

// Request interception over the CDP Fetch domain. Every paused request must be
// answered exactly once or the page hangs; requests matching no rule continue
// unmodified.

// ActionKind says what to do with a request whose URL matches a rule.
type ActionKind int

const (
	// Watch lets the request through untouched and only counts it.
	Watch ActionKind = iota
	// Abort refuses it, as an ad blocker or an offline network would.
	Abort
	// Delay holds it for a while, then lets it through.
	Delay
	// Hold keeps it until released: Delay without the clock.
	Hold
	// Fulfill answers it here, without going to the network.
	Fulfill
)

// Action is a rule's behaviour.
type Action struct {
	Kind        ActionKind
	Delay       time.Duration
	ContentType string
	Body        string
}

type rule struct {
	pattern *regexp.Regexp
	// method restricts the rule to one HTTP method when set.
	method  string
	action  Action
	release chan struct{}
	arrived atomic.Int64
	hits    atomic.Int64
}

// Document is one navigation's URL and status code.
type Document struct {
	URL    string
	Status int64
}

// Network is the interception of one browser: request rules plus a log of
// top-level document statuses.
type Network struct {
	browser   *Browser
	mu        sync.Mutex
	rules     []*rule
	documents []Document
}

// AttachNetwork starts intercepting every request of the browser.
func (b *Browser) AttachNetwork() (*Network, error) {
	n := &Network{browser: b}
	// Listening before enabling: a request paused with no listener stalls the
	// page.
	if err := chromedp.Listen(b.ctx, n.onEvent); err != nil {
		return nil, fmt.Errorf("listening for requests: %w", err)
	}
	if err := send(b, network.Enable, network.EnableParams{}); err != nil {
		return nil, fmt.Errorf("enabling network events: %w", err)
	}
	if err := send(b, fetch.Enable, fetch.EnableParams{Patterns: []*fetch.RequestPattern{{URLPattern: "*"}}}); err != nil {
		return nil, fmt.Errorf("enabling request interception: %w", err)
	}
	return n, nil
}

func (n *Network) onEvent(event any) {
	switch event := event.(type) {
	case *network.EventResponseReceived:
		if event.Type != network.ResourceTypeDocument {
			return
		}
		n.mu.Lock()
		n.documents = append(n.documents, Document{URL: event.Response.URL, Status: event.Response.Status})
		n.mu.Unlock()
	case *fetch.EventRequestPaused:
		// Its own goroutine per request, so a held one does not stall others.
		go n.answer(event)
	}
}

func (n *Network) match(url, method string) *rule {
	n.mu.Lock()
	defer n.mu.Unlock()
	for _, candidate := range n.rules {
		if candidate.pattern.MatchString(url) && (candidate.method == "" || candidate.method == method) {
			return candidate
		}
	}
	return nil
}

func (n *Network) answer(event *fetch.EventRequestPaused) {
	matched := n.match(event.Request.URL, event.Request.Method)
	if matched == nil {
		n.continueRequest(event)
		return
	}
	// Announced before any hold, so "request made" and "response landed" can be
	// awaited separately.
	matched.arrived.Add(1)
	n.respond(matched, event)
	matched.hits.Add(1)
}

func (n *Network) respond(r *rule, event *fetch.EventRequestPaused) {
	// Errors are dropped: a late answer fails harmlessly once the page is gone
	// or its stale-response guard aborted the request.
	ctx := n.browser.ctx
	switch r.action.Kind {
	case Abort:
		_, _ = chromedp.Call(ctx, fetch.FailRequest, fetch.FailRequestParams{
			RequestID: event.RequestID, ErrorReason: network.ErrorReasonBlockedByClient,
		})
	case Delay:
		time.Sleep(r.action.Delay)
		n.continueRequest(event)
	case Hold:
		select {
		case <-r.release:
		case <-ctx.Done():
		}
		n.continueRequest(event)
	case Fulfill:
		_, _ = chromedp.Call(ctx, fetch.FulfillRequest, fetch.FulfillRequestParams{
			RequestID:       event.RequestID,
			ResponseCode:    200,
			ResponseHeaders: []*fetch.HeaderEntry{{Name: "Content-Type", Value: r.action.ContentType}},
			Body:            []byte(r.action.Body),
		})
	default:
		n.continueRequest(event)
	}
}

func (n *Network) continueRequest(event *fetch.EventRequestPaused) {
	_, _ = chromedp.Call(n.browser.ctx, fetch.ContinueRequest, fetch.ContinueRequestParams{RequestID: event.RequestID})
}

// DocumentStatus is the status of the most recent navigation to a URL
// containing needle; false when there was none.
func (n *Network) DocumentStatus(needle string) (int64, bool) {
	n.mu.Lock()
	defer n.mu.Unlock()
	for i := len(n.documents) - 1; i >= 0; i-- {
		if strings.Contains(n.documents[i].URL, needle) {
			return n.documents[i].Status, true
		}
	}
	return 0, false
}

// Route adds a rule for every method. Rules are tried in order; the first
// match wins.
func (n *Network) Route(pattern string, action Action) (*RouteHandle, error) {
	return n.RouteMethod(pattern, "", action)
}

// RouteMethod adds a rule scoped to one HTTP method.
func (n *Network) RouteMethod(pattern, method string, action Action) (*RouteHandle, error) {
	compiled, err := regexp.Compile(pattern)
	if err != nil {
		return nil, fmt.Errorf("`%s` is not a valid URL pattern: %w", pattern, err)
	}
	added := &rule{pattern: compiled, method: method, action: action, release: make(chan struct{})}
	n.mu.Lock()
	n.rules = append(n.rules, added)
	n.mu.Unlock()
	return &RouteHandle{rule: added}, nil
}

// HoldRequests holds every matching request until the handle is released.
func (n *Network) HoldRequests(pattern string) (*RouteHandle, error) {
	return n.Route(pattern, Action{Kind: Hold})
}

// RouteHandle is a rule's counters, plus the release switch of a hold.
type RouteHandle struct {
	rule        *rule
	releaseOnce sync.Once
}

// Arrived counts the matching requests intercepted, held or not.
func (h *RouteHandle) Arrived() int { return int(h.rule.arrived.Load()) }

// Hits counts the matching requests answered.
func (h *RouteHandle) Hits() int { return int(h.rule.hits.Load()) }

// WaitForArrival waits until a matching request has been intercepted.
func (h *RouteHandle) WaitForArrival() error {
	return Eventually("a matching request to be made", func() (bool, error) { return h.Arrived() > 0, nil })
}

// WaitForSettled waits until a matching request has been answered.
func (h *RouteHandle) WaitForSettled() error {
	return Eventually("a held request to settle", func() (bool, error) { return h.Hits() > 0, nil })
}

// Release lets held requests through, now and later.
func (h *RouteHandle) Release() error {
	if h.rule.action.Kind != Hold {
		return fmt.Errorf("this route was not created with HoldRequests")
	}
	h.releaseOnce.Do(func() { close(h.rule.release) })
	return nil
}
