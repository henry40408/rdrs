package main

import (
	"bytes"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"io/fs"
	"log/slog"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func TestJSONFormatIsParseable(t *testing.T) {
	var out bytes.Buffer
	newLogger(loggerSettings{format: formatJSON, level: slog.LevelInfo}, &out).Info("hello", "event", "x.y", "count", 3)

	var got map[string]any
	if err := json.Unmarshal(out.Bytes(), &got); err != nil {
		t.Fatalf("%v: %q", err, out.String())
	}
	if got["level"] != "INFO" || got["msg"] != "hello" || got["event"] != "x.y" || got["count"] != float64(3) || got["time"] == nil {
		t.Fatalf("%+v", got)
	}
}

func TestEveryTextFormatPrintsText(t *testing.T) {
	for _, format := range []logFormat{formatFull, formatCompact, formatPretty} {
		var out bytes.Buffer
		newLogger(loggerSettings{format: format, level: slog.LevelInfo}, &out).Warn("careful", "event", "config.warning")
		for _, want := range []string{"level=WARN", `msg=careful`, "event=config.warning"} {
			if !strings.Contains(out.String(), want) {
				t.Errorf("%s: missing %q in %q", format, want, out.String())
			}
		}
	}
}

func TestLevelFilter(t *testing.T) {
	var out bytes.Buffer
	logger := newLogger(loggerSettings{format: formatFull, level: slog.LevelWarn}, &out)
	logger.Info("quiet", "event", "x")
	logger.Warn("loud", "event", "x")
	if strings.Contains(out.String(), "quiet") || !strings.Contains(out.String(), "loud") {
		t.Fatal(out.String())
	}
}

func TestParseLogFormat(t *testing.T) {
	if _, err := parseLogFormat("bogus"); err == nil {
		t.Fatal("want error")
	}
}

var logMethods = map[string]bool{"Debug": true, "Info": true, "Warn": true, "Error": true}

// TestEveryLogCallCarriesAnEvent is the structured-logging lint: a literal
// message must come with an "event" key, and must not interpolate values.
// A new call site written the old way would never fail at run time.
func TestEveryLogCallCarriesAnEvent(t *testing.T) {
	root := filepath.Join("..", "..")
	calls := 0
	fset := token.NewFileSet()
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() && (d.Name() == "vendor" || d.Name() == ".claude" || d.Name() == "e2e" || d.Name() == "target") {
			return filepath.SkipDir
		}
		if d.IsDir() || !strings.HasSuffix(path, ".go") || strings.HasSuffix(path, "_test.go") {
			return nil
		}
		file, err := parser.ParseFile(fset, path, nil, 0)
		if err != nil {
			return err
		}
		ast.Inspect(file, func(n ast.Node) bool {
			call, ok := n.(*ast.CallExpr)
			if !ok {
				return true
			}
			if checkLogCall(t, fset, call) {
				calls++
			}
			return true
		})
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	// A scanner that found nothing would pass vacuously.
	if calls < 3 {
		t.Fatalf("only %d log calls found; the scanner is probably broken", calls)
	}
}

// checkLogCall reports whether call is a logger call, failing the test when it
// breaks the convention.
func checkLogCall(t *testing.T, fset *token.FileSet, call *ast.CallExpr) bool {
	sel, ok := call.Fun.(*ast.SelectorExpr)
	if !ok || !logMethods[sel.Sel.Name] || len(call.Args) == 0 {
		return false
	}
	receiver, ok := sel.X.(*ast.Ident)
	if !ok || (receiver.Name != "logger" && receiver.Name != "slog" && receiver.Name != "log") {
		return false
	}
	pos := fset.Position(call.Pos())
	if msg, ok := stringLiteral(call.Args[0]); ok && strings.ContainsAny(msg, "%{}") {
		t.Errorf("%s: log message interpolates values; put them in fields", pos)
	}
	if !hasEventKey(call.Args[1:]) {
		t.Errorf("%s: log call has no \"event\" field", pos)
	}
	return true
}

func stringLiteral(e ast.Expr) (string, bool) {
	lit, ok := e.(*ast.BasicLit)
	if !ok || lit.Kind != token.STRING {
		return "", false
	}
	s, err := strconv.Unquote(lit.Value)
	return s, err == nil
}

func hasEventKey(args []ast.Expr) bool {
	for _, arg := range args {
		if s, ok := stringLiteral(arg); ok && s == "event" {
			return true
		}
	}
	return false
}
