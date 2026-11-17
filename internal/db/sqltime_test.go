package db

import (
	"testing"
	"time"
)

func TestFormatSpace(t *testing.T) {
	zone := time.FixedZone("x", 8*3600)
	got := FormatSpace(time.Date(2026, 11, 17, 8, 9, 10, 999, zone))
	if got != "2026-11-17 00:09:10" {
		t.Fatalf("FormatSpace = %q", got)
	}
}

func TestFormatRustRFC3339(t *testing.T) {
	base := time.Date(2026, 11, 17, 7, 9, 47, 0, time.UTC)
	cases := []struct {
		nanos int
		want  string
	}{
		{0, "2026-11-17T07:09:47+00:00"},
		{500_000_000, "2026-11-17T07:09:47.500+00:00"},
		{640_715_000, "2026-11-17T07:09:47.640715+00:00"},
		{640_715_123, "2026-11-17T07:09:47.640715123+00:00"},
		{1_000, "2026-11-17T07:09:47.000001+00:00"},
	}
	for _, tc := range cases {
		got := FormatRustRFC3339(base.Add(time.Duration(tc.nanos)))
		if got != tc.want {
			t.Errorf("nanos %d: got %q, want %q", tc.nanos, got, tc.want)
		}
	}
}

func TestParseStored(t *testing.T) {
	want := time.Date(2026, 11, 17, 7, 9, 47, 0, time.UTC)
	cases := []struct {
		name string
		in   any
		want time.Time
	}{
		{"chrono micros", "2026-11-17T07:09:47.640715+00:00", want.Add(640_715 * time.Microsecond)},
		{"chrono whole seconds", "2026-11-17T07:09:47+00:00", want},
		{"zulu", "2026-11-17T07:09:47Z", want},
		{"offset", "2026-11-17T15:09:47+08:00", want},
		{"space", "2026-11-17 07:09:47", want},
		{"space fraction", "2026-11-17 07:09:47.250", want.Add(250 * time.Millisecond)},
		{"space minutes", "2026-11-17 07:09", want.Add(-47 * time.Second)},
		{"T no zone", "2026-11-17T07:09:47", want},
		{"T no zone fraction", "2026-11-17T07:09:47.5", want.Add(500 * time.Millisecond)},
		{"space with zone", "2026-11-17 07:09:47+00:00", want},
		{"space minutes zulu", "2026-11-17 07:09Z", want.Add(-47 * time.Second)},
		{"space minutes offset", "2026-11-17 15:09+08:00", want.Add(-47 * time.Second)},
		{"T minutes zulu", "2026-11-17T07:09Z", want.Add(-47 * time.Second)},
		{"T minutes offset", "2026-11-17T15:09+08:00", want.Add(-47 * time.Second)},
		{"bytes", []byte("2026-11-17 07:09:47"), want},
		{"unix int", want.Unix(), want},
		{"unix text", "1794035387", time.Unix(1794035387, 0).UTC()},
		{"julian", float64(want.Unix())/86400 + 2440587.5, want},
		{"time.Time", want.In(time.FixedZone("x", 3600)), want},
	}
	for _, tc := range cases {
		got, err := ParseStored(tc.in)
		if err != nil {
			t.Errorf("%s: %v", tc.name, err)
			continue
		}
		if diff := got.Sub(tc.want); diff > time.Millisecond || diff < -time.Millisecond {
			t.Errorf("%s: got %v, want %v", tc.name, got, tc.want)
		}
		if got.Location() != time.UTC {
			t.Errorf("%s: location %v, want UTC", tc.name, got.Location())
		}
	}
}

func TestParseStoredRejectsGarbage(t *testing.T) {
	for _, in := range []any{"not a time", "", nil, true} {
		if _, err := ParseStored(in); err == nil {
			t.Errorf("ParseStored(%#v) succeeded", in)
		}
	}
}

func TestRustFormatRoundTrips(t *testing.T) {
	in := time.Date(2026, 1, 2, 3, 4, 5, 123_456_789, time.UTC)
	got, err := ParseStored(FormatRustRFC3339(in))
	if err != nil || !got.Equal(in) {
		t.Fatalf("round trip = %v, %v", got, err)
	}
}
