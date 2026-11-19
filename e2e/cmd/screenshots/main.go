// Command screenshots regenerates the four screenshots README.md embeds, into
// ../screenshots/.
//
//	cd e2e && go run -mod=vendor ./cmd/screenshots
//
// Font rendering and Chromium version differ by machine; an unrelated image
// changing in the diff means the browser changed, not the UI.
package main

import (
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

const (
	// demoUser is created via the setup endpoint so it is admin; an invited
	// member would lack the admin sidebar entries.
	demoUser = "demouser"
	// Wide enough for the three-pane layout.
	viewportWidth  = 1920
	viewportHeight = 1080
	// faviconTimeout is how long a favicon fetch may take; past it the feed
	// goes without.
	faviconTimeout = 5 * time.Second
	// faviconMaxBytes: anything larger is not a favicon.
	faviconMaxBytes = 256 * 1024
	// fadeIn is the help overlay's fade; capturing on its class alone catches
	// it mid-fade.
	fadeIn = 200 * time.Millisecond
)

var slugCharacters = regexp.MustCompile(`[^a-z0-9]`)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run() error {
	output := filepath.Join(harness.RepoRoot(), "screenshots")
	if err := os.MkdirAll(output, 0o755); err != nil {
		return err
	}
	server, err := harness.Start()
	if err != nil {
		return err
	}
	defer server.Stop()
	if err := seedDemoContent(server); err != nil {
		return err
	}
	for _, theme := range []string{"light", "dark"} {
		if err := capture(server, theme, output); err != nil {
			return err
		}
	}
	fmt.Printf("screenshots: wrote four images to %s\n", output)
	return nil
}

type icon struct {
	data        []byte
	contentType string
}

func seedDemoContent(server *harness.Server) error {
	if err := harness.NewAPI(server).SetupFirstAccount(demoUser, harness.Password); err != nil {
		return err
	}
	seed, err := harness.OpenSeed(server.DBPath)
	if err != nil {
		return err
	}
	defer seed.Close()
	userID, err := seed.UserID(demoUser)
	if err != nil {
		return err
	}
	// A missing icon falls back to the initial chip, which is fine.
	icons := fetchFavicons()
	hour := 1
	for _, group := range demoContent {
		categoryID, err := seed.CreateCategory(userID, group.name)
		if err != nil {
			return err
		}
		for _, item := range group.feeds {
			if err := seedFeed(seed, categoryID, item, icons[item.icon], &hour); err != nil {
				return err
			}
		}
	}
	return nil
}

func seedFeed(seed *harness.Seed, categoryID int64, item feed, favicon *icon, hour *int) error {
	feedID, err := seed.CreateFeed(categoryID, item.url, item.title)
	if err != nil {
		return err
	}
	if favicon != nil {
		if err := seed.InsertIcon(feedID, favicon.data, favicon.contentType, item.icon); err != nil {
			return err
		}
	}
	origin, _, _ := strings.Cut(item.url, "/feed")
	entries := make([]harness.NewEntry, 0, len(item.entries))
	for index, shown := range item.entries {
		slug := slugCharacters.ReplaceAllString(strings.ToLower(shown.title), "-")
		entries = append(entries, harness.NewEntry{
			FeedID:          feedID,
			GUID:            fmt.Sprintf("%s/entry-%d", item.url, index),
			Title:           shown.title,
			Link:            origin + "/" + slug,
			Content:         shown.content,
			PublishedOffset: fmt.Sprintf("-%d hours", *hour),
		})
		*hour++
	}
	_, err = seed.InsertEntries(entries)
	return err
}

// fetchFavicons fetches every favicon concurrently, keeping the ones that
// arrived.
func fetchFavicons() map[string]*icon {
	client := &http.Client{Timeout: faviconTimeout}
	var mu sync.Mutex
	var wg sync.WaitGroup
	icons := map[string]*icon{}
	for _, group := range demoContent {
		for _, item := range group.feeds {
			wg.Add(1)
			go func() {
				defer wg.Done()
				fetched := fetchFavicon(client, item.icon)
				if fetched == nil {
					return
				}
				mu.Lock()
				icons[item.icon] = fetched
				mu.Unlock()
			}()
		}
	}
	wg.Wait()
	return icons
}

func fetchFavicon(client *http.Client, url string) *icon {
	response, err := client.Get(url)
	if err != nil {
		return nil
	}
	defer func() { _ = response.Body.Close() }()
	if response.StatusCode < 200 || response.StatusCode > 299 {
		return nil
	}
	data, err := io.ReadAll(io.LimitReader(response.Body, faviconMaxBytes+1))
	if err != nil || len(data) == 0 || len(data) > faviconMaxBytes {
		return nil
	}
	contentType := response.Header.Get("Content-Type")
	if contentType == "" {
		contentType = "image/x-icon"
	}
	return &icon{data: data, contentType: contentType}
}

// capture signs in and captures both images for one colour scheme.
func capture(server *harness.Server, theme, output string) error {
	browser, err := harness.Open()
	if err != nil {
		return err
	}
	defer browser.Close()
	if err := browser.SetViewport(viewportWidth, viewportHeight); err != nil {
		return err
	}
	// Emulated, to show the system-follow theme path.
	if err := browser.EmulateColorScheme(theme); err != nil {
		return err
	}
	suffix := ""
	if theme == "dark" {
		suffix = "-dark"
	}
	if err := signIn(server, browser); err != nil {
		return err
	}
	if err := openFirstEntry(browser); err != nil {
		return err
	}
	if err := writePNG(browser, filepath.Join(output, "unread-list"+suffix+".png")); err != nil {
		return err
	}
	if err := openHelpOverlay(browser); err != nil {
		return err
	}
	return writePNG(browser, filepath.Join(output, "keyboard-shortcuts"+suffix+".png"))
}

func signIn(server *harness.Server, browser *harness.Browser) error {
	if err := browser.Goto(server.BaseURL + "/login"); err != nil {
		return err
	}
	if err := browser.Fill("username-input", demoUser); err != nil {
		return err
	}
	if err := browser.Fill("password-input", harness.Password); err != nil {
		return err
	}
	if err := browser.Submit("login-submit"); err != nil {
		return err
	}
	if err := browser.ExpectVisible("entry-item"); err != nil {
		return err
	}
	// Wait for icons to load or fail, or the sidebar is half-drawn.
	return harness.Eventually("the feed icons to settle", func() (bool, error) {
		return harness.Eval[bool](browser, `[...document.querySelectorAll('.feed-icon')].every((img) => img.complete)`)
	})
}

func unreadCount(browser *harness.Browser) (string, error) {
	badge, err := browser.CSS("#unread-count")
	if err != nil {
		return "", err
	}
	text, err := badge.TextContent()
	return strings.TrimSpace(text), err
}

// openFirstEntry presses j to select the first row and o to open it.
func openFirstEntry(browser *harness.Browser) error {
	text, err := unreadCount(browser)
	if err != nil {
		return err
	}
	before, _ := strconv.Atoi(text)
	if err := browser.Press("j"); err != nil {
		return err
	}
	if err := browser.PressFocused("o"); err != nil {
		return err
	}
	if _, err := browser.CSS(".reading-pane-title"); err != nil {
		return err
	}
	// Opening marks read, and the badge updates later via SSE and a debounce;
	// wait for it so the screenshot is consistent.
	return harness.EventuallyEq("the unread badge", strconv.Itoa(before-1), func() (string, error) {
		return unreadCount(browser)
	})
}

func openHelpOverlay(browser *harness.Browser) error {
	if err := browser.Press("?"); err != nil {
		return err
	}
	err := harness.Eventually("the help overlay to open", func() (bool, error) {
		overlay, err := browser.CSSOpt("rdrs-kb-help")
		if err != nil || overlay == nil {
			return false, err
		}
		classes, err := overlay.Classes()
		return contains(classes, "visible"), err
	})
	time.Sleep(fadeIn)
	return err
}

func contains(values []string, wanted string) bool {
	for _, value := range values {
		if value == wanted {
			return true
		}
	}
	return false
}

func writePNG(browser *harness.Browser, path string) error {
	png, err := browser.Screenshot()
	if err != nil {
		return err
	}
	return os.WriteFile(path, png, 0o644)
}
