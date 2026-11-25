// Package harness drives an rdrs server and a browser for the e2e suite.
package harness

import (
	"bufio"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"
)

const startupTimeout = 30 * time.Second

const setupTokenMarker = "setup_token="

// BinaryEnv names the variable that pins the server binary under test.
const BinaryEnv = "RDRS_BIN"

// Server is an rdrs server with its own data directory and its two mock
// upstreams. Every scenario starts one, so scenarios share no state.
type Server struct {
	// BaseURL is where the browser goes.
	BaseURL string
	// DBPath is the SQLite file the server runs on, for direct seeding.
	DBPath string
	// FeedURL always answers with a valid RSS document.
	FeedURL string
	// SetupToken is the first-run token the server printed to stderr.
	SetupToken string

	cmd     *exec.Cmd
	mocks   []*mock
	dataDir string
	stdout  *collector
}

// Start runs the rdrs binary, building it first when it is missing.
func Start() (*Server, error) {
	binary, err := ensureBinary()
	if err != nil {
		return nil, err
	}
	dataDir, err := os.MkdirTemp("", "rdrs-e2e-")
	if err != nil {
		return nil, err
	}
	// The mocks bind first and keep their ports, so freePort cannot hand one of
	// them to rdrs. The other order let a mock take the freed port: rdrs then
	// failed to bind, and the mock answered /health and POST /setup in its place.
	feed, kagi := startFeedMock(), startKagiMock()
	server := &Server{
		DBPath:  filepath.Join(dataDir, "test.sqlite3"),
		FeedURL: feed.URL,
		mocks:   []*mock{feed, kagi},
		dataDir: dataDir,
	}
	port, err := freePort()
	if err != nil {
		server.Stop()
		return nil, err
	}
	server.BaseURL = "http://127.0.0.1:" + strconv.Itoa(port)
	if err := server.launch(binary, port, kagi.URL); err != nil {
		server.Stop()
		return nil, err
	}
	return server, nil
}

func (s *Server) launch(binary string, port int, kagiURL string) error {
	cmd := exec.Command(binary)
	cmd.Dir = RepoRoot()
	cmd.Env = append(os.Environ(),
		"DATABASE_URL="+s.DBPath,
		"RDRS_SERVER_BIND=127.0.0.1:"+strconv.Itoa(port),
		"RDRS_MULTI_USER_ENABLED=true",
		"RDRS_LOG_LEVEL=warn",
		// Minimal Argon2 cost for per-scenario logins. Never in production.
		"RDRS_FAST_HASH=1",
		// Every scenario logs in from 127.0.0.1; the limiter would refuse
		// logins a few scenarios in. It is covered by the integration tests.
		"RDRS_LOGIN_RATE_LIMIT_ATTEMPTS=0",
		// Direct SQLite seeding skips the cache's bust hooks, so a render
		// mid-seed would cache a half-seeded sidebar for the whole TTL.
		"RDRS_DISABLE_SIDEBAR_CACHE=1",
		"RDRS_KAGI_API_BASE="+kagiURL,
		// The mock feed binds loopback, which the SSRF guard refuses by
		// default; this is the self-hoster LAN opt-in, not a test bypass.
		"RDRS_FETCH_ALLOW_PRIVATE_HOSTS=127.0.0.1",
	)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("spawning %s: %w", binary, err)
	}
	s.cmd = cmd
	s.stdout = forwardLog(stdout, os.Stdout)
	tokens := forwardStderrAndFindToken(stderr)
	if err := s.waitUntilHealthy(); err != nil {
		return err
	}
	// Printed before the listener binds, so it has arrived once /health answers.
	select {
	case s.SetupToken = <-tokens:
	case <-time.After(5 * time.Second):
		return fmt.Errorf("the server did not print a setup token")
	}
	// The log is what operators ship to collectors; the token must stay out of
	// it. The reader may lag the server, so this is best effort.
	if s.stdout.contains(s.SetupToken) {
		return fmt.Errorf("the setup token leaked into the log on stdout")
	}
	return nil
}

// Stop kills the server and removes its data. Safe to call twice.
func (s *Server) Stop() {
	if s.cmd != nil {
		_ = s.cmd.Process.Kill()
		_ = s.cmd.Wait()
		s.cmd = nil
	}
	for _, m := range s.mocks {
		m.Close()
	}
	s.mocks = nil
	_ = os.RemoveAll(s.dataDir)
}

// collector keeps a copy of what it forwards.
type collector struct {
	mu   sync.Mutex
	text strings.Builder
}

func (c *collector) contains(needle string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return strings.Contains(c.text.String(), needle)
}

// forwardLog echoes a stream to out and keeps a copy.
func forwardLog(stream io.Reader, out io.Writer) *collector {
	log := &collector{}
	go func() {
		lines := bufio.NewScanner(stream)
		for lines.Scan() {
			_, _ = fmt.Fprintln(out, lines.Text())
			log.mu.Lock()
			log.text.WriteString(lines.Text() + "\n")
			log.mu.Unlock()
		}
	}()
	return log
}

// forwardStderrAndFindToken echoes stderr and delivers the setup token it
// prints (setup_token=<token>). It keeps forwarding afterwards so the server
// never blocks on a full pipe.
func forwardStderrAndFindToken(stderr io.Reader) <-chan string {
	tokens := make(chan string, 1)
	go func() {
		lines := bufio.NewScanner(stderr)
		for lines.Scan() {
			line := lines.Text()
			fmt.Fprintln(os.Stderr, line)
			for _, word := range strings.Fields(line) {
				token, found := strings.CutPrefix(word, setupTokenMarker)
				if !found {
					continue
				}
				select {
				case tokens <- token:
				default:
				}
			}
		}
	}()
	return tokens
}

func (s *Server) waitUntilHealthy() error {
	deadline := time.Now().Add(startupTimeout)
	for time.Now().Before(deadline) {
		response, err := http.Get(s.BaseURL + "/health")
		if err == nil {
			_ = response.Body.Close()
			if response.StatusCode == http.StatusOK {
				return nil
			}
		}
		time.Sleep(100 * time.Millisecond)
	}
	return fmt.Errorf("rdrs did not answer %s/health within %v", s.BaseURL, startupTimeout)
}

func freePort() (int, error) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 0, fmt.Errorf("probing for a free port: %w", err)
	}
	defer func() { _ = listener.Close() }()
	return listener.Addr().(*net.TCPAddr).Port, nil
}

// ensureBinary returns the server binary: RDRS_BIN, else bin/rdrs, built first
// when it is not there.
//
// An existing binary is never rebuilt, so rebuild by hand after changing
// server code: the web files are embedded, a stale binary shows stale UI.
func ensureBinary() (string, error) {
	if pinned := os.Getenv(BinaryEnv); pinned != "" {
		if !isFile(pinned) {
			return "", fmt.Errorf("%s=%s is not a file", BinaryEnv, pinned)
		}
		return filepath.Abs(pinned)
	}
	binary := filepath.Join(RepoRoot(), "bin", "rdrs")
	if isFile(binary) {
		return binary, nil
	}
	fmt.Fprintf(os.Stderr, "e2e: %s is missing, building it\n", binary)
	build := exec.Command("go", "build", "-mod=vendor", "-o", "bin/rdrs", "./cmd/rdrs")
	build.Dir = RepoRoot()
	build.Stdout = os.Stdout
	build.Stderr = os.Stderr
	if err := build.Run(); err != nil {
		return "", fmt.Errorf("running go build: %w", err)
	}
	return binary, nil
}

func isFile(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}

// RepoRoot is the repository root, three levels above e2e/internal/harness. It
// comes from this file's compile-time path, so the suite works from any
// working directory.
func RepoRoot() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..", "..", "..")
}
