package main

import "testing"

func TestTakeOutControlKeepsOtherViolations(t *testing.T) {
	control := []violation{
		{Directive: "style-src-elem", Sample: "planted"},
		{Directive: "script-src", Sample: "real"},
		{Directive: "style-src-attr", Sample: "second style"},
	}
	rest, err := takeOutControl(control)
	if err != nil {
		t.Fatal(err)
	}
	if len(rest) != 2 || rest[0].Sample != "real" || rest[1].Sample != "second style" {
		t.Fatalf("rest = %+v", rest)
	}
}

func TestTakeOutControlSkipsLeadingNonStyleViolation(t *testing.T) {
	rest, err := takeOutControl([]violation{{Directive: "img-src"}, {Directive: "style-src"}})
	if err != nil || len(rest) != 1 || rest[0].Directive != "img-src" {
		t.Fatalf("rest = %+v, err = %v", rest, err)
	}
}

func TestTakeOutControlFailsWithoutAStyleViolation(t *testing.T) {
	if _, err := takeOutControl(nil); err == nil {
		t.Fatal("an empty control must fail")
	}
	if _, err := takeOutControl([]violation{{Directive: "img-src"}}); err == nil {
		t.Fatal("a control without style-src must fail")
	}
}

func TestVerdictReportsEveryRemainingViolation(t *testing.T) {
	if err := (&audit{}).verdict(); err != nil {
		t.Fatal(err)
	}
	failing := &audit{violations: []violation{{Directive: "script-src", Where: "positive control"}}}
	if failing.verdict() == nil {
		t.Fatal("a violation must fail the audit")
	}
}
