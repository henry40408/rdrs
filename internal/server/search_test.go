package server

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestHighlightEscapesAndMergesMatches(t *testing.T) {
	cases := []struct {
		text  string
		terms []string
		want  string
	}{
		{"a <b> & c", nil, "a &lt;b&gt; &amp; c"},
		{"Rust and rust", []string{"rust"}, "<mark>Rust</mark> and <mark>rust</mark>"},
		{"foobar", []string{"foo", "oba"}, "<mark>fooba</mark>r"},
		{"<i>x</i>", []string{"i"}, "&lt;<mark>i</mark>&gt;x&lt;/<mark>i</mark>&gt;"},
		// Only ASCII letters fold, so İ is not a match for i.
		{"İ", []string{"i"}, "İ"},
		{"x", []string{""}, "x"},
	}
	for _, c := range cases {
		if got := highlightHTML(c.text, c.terms); got != c.want {
			t.Errorf("highlightHTML(%q, %q) = %q, want %q", c.text, c.terms, got, c.want)
		}
	}
}

func TestSnippetCentresOnTheFirstMatch(t *testing.T) {
	long := strings.Repeat("a ", 200) + "needle " + strings.Repeat("b ", 200)
	got := buildSnippet(&long, []string{"NEEDLE"}, 40)
	if !strings.HasPrefix(got, "…") || !strings.HasSuffix(got, "…") || !strings.Contains(got, "needle") {
		t.Errorf("snippet = %q", got)
	}
	if got := buildSnippet(&long, []string{"absent"}, 10); got != "a a a a a…" {
		t.Errorf("no match: %q", got)
	}
	short := "<p>tiny</p>"
	if got := buildSnippet(&short, nil, 200); got != strings.TrimSpace(got) || !strings.Contains(got, "tiny") {
		t.Errorf("short: %q", got)
	}
	if buildSnippet(nil, nil, 10) != "" {
		t.Error("nil body must give an empty snippet")
	}
}

func TestSearchPageModes(t *testing.T) {
	f := newSignedIn(t)
	f.withEntries(t)
	page := f.get("/search?q=title")
	if page.Code != http.StatusOK || !strings.Contains(page.Body.String(), `data-testid="search-results"`) ||
		!strings.Contains(page.Body.String(), "<mark>Title</mark> a") {
		t.Errorf("page: %d %s", page.Code, page.Body)
	}
	refresh := f.get("/search?q=title&fragment=1")
	if !strings.HasPrefix(refresh.Body.String(), `<template data-swap-target="[data-search-results]">`) {
		t.Errorf("refresh: %.120s", refresh.Body)
	}
	more := f.get("/search?q=title&fragment=1&after=")
	if !strings.HasPrefix(more.Body.String(), `<template data-swap-target="#search-load-more">`) {
		t.Errorf("load more: %.120s", more.Body)
	}
	document := httptest.NewRequest(http.MethodGet, "/search?q=title&fragment=1", nil)
	document.Header.Set("Sec-Fetch-Dest", "document")
	document.AddCookie(f.session)
	if body := f.do(document).Body.String(); !strings.HasPrefix(body, "<!DOCTYPE html>") {
		t.Errorf("document navigation got a fragment: %.80s", body)
	}
	if rec := f.get("/search?fragment=x"); rec.Code != http.StatusBadRequest {
		t.Errorf("bad fragment: %d", rec.Code)
	}
}

func TestSearchPageRequiresLogin(t *testing.T) {
	f := newAuthFixture(t)
	rec := f.do(httptest.NewRequest(http.MethodGet, "/search?q=x", nil))
	if rec.Code != http.StatusSeeOther || !strings.HasPrefix(rec.Header().Get("Location"), "/login") {
		t.Errorf("%d %q", rec.Code, rec.Header().Get("Location"))
	}
}
