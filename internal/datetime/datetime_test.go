package datetime

import (
	"testing"
	"time"
)

const rfc3339UTC = "2006-01-02T15:04:05+00:00"

func format(t time.Time) string { return t.UTC().Format(rfc3339UTC) }

func TestTryParseSupportedFormats(t *testing.T) {
	a, ok1 := TryParse("2026-01-06 14:28:00")
	b, ok2 := TryParse("2026-01-06T14:28:00Z")
	if !ok1 || !ok2 || !a.Equal(b) {
		t.Fatalf("SQL and RFC 3339 differ: %v %v", a, b)
	}
	if got := format(a); got != "2026-01-06T14:28:00+00:00" {
		t.Errorf("got %s", got)
	}
}

func TestTryParseGarbageIsNone(t *testing.T) {
	for _, s := range []string{"not a date", "", "   ", "12:00"} {
		if _, ok := TryParse(s); ok {
			t.Errorf("TryParse(%q) succeeded", s)
		}
	}
}

func TestTryParseWithoutOffsetIsUTC(t *testing.T) {
	got, ok := TryParse("Jan 6, 2026 14:28:00")
	if !ok || format(got) != "2026-01-06T14:28:00+00:00" {
		t.Errorf("got %v %v", got, ok)
	}
}

func TestParseOrNowFallsBack(t *testing.T) {
	got := ParseOrNow("totally unparsable")
	if diff := time.Since(got); diff < 0 || diff > 5*time.Second {
		t.Errorf("not near now: %v", got)
	}
}

func TestParseTimestamp(t *testing.T) {
	cases := []struct{ in, want string }{
		{"Thu, 22 Jan 2026 15:09:47 +08:00", "2026-01-22T07:09:47+00:00"},
		{"Thu, 22 Jan 2026 15:09:47 +0800", "2026-01-22T07:09:47+00:00"},
		{"2026-01-22T15:09:47+08:00", "2026-01-22T07:09:47+00:00"},
		{"週四, 22 一月 2026 15:09:47 +0800", "2026-01-22T07:09:47+00:00"},
		{"2026-01-06 14:28:00", "2026-01-06T14:28:00+00:00"},
		{"2026-01-06", "2026-01-06T00:00:00+00:00"},
		{"Tue, 06 Jan 2026 14:28:00 +0800", "2026-01-06T06:28:00+00:00"},
		{"Tue, 06 Jan 2026 14:28:00 +08:00", "2026-01-06T06:28:00+00:00"},
		{"6 一月 2026 14:28:00 +0000", "2026-01-06T14:28:00+00:00"},
		{"週二, 6 一月 2026 14:28:00", "2026-01-06T14:28:00+00:00"},
	}
	for _, tc := range cases {
		got, ok := ParseTimestamp(tc.in)
		if !ok || format(got) != tc.want {
			t.Errorf("ParseTimestamp(%q) = %v, %v; want %s", tc.in, got, ok, tc.want)
		}
	}
	if _, ok := ParseTimestamp("not a date"); ok {
		t.Error("garbage parsed")
	}
}

// Shapes from the dateparser README, with the expected UTC reading.
func TestLooseFormats(t *testing.T) {
	cases := []struct{ in, want string }{
		{"1511648546", "2017-11-25T22:22:26+00:00"},
		{"1620021848429", "2021-05-03T06:04:08+00:00"},
		{"1620024872717915000", "2021-05-03T06:54:32+00:00"},
		{"2021-05-01T01:17:02.604456Z", "2021-05-01T01:17:02+00:00"},
		{"Wed, 02 Jun 2021 06:31:39 GMT", "2021-06-02T06:31:39+00:00"},
		{"2019-11-29 08:08-08", "2019-11-29T16:08:00+00:00"},
		{"2019-11-29 08:08:05-08", "2019-11-29T16:08:05+00:00"},
		{"2021-05-02 23:31:36.0741-07", "2021-05-03T06:31:36+00:00"},
		{"2017-07-19 03:21:51+00:00", "2017-07-19T03:21:51+00:00"},
		{"2014-04-26 05:24:37 PM", "2014-04-26T17:24:37+00:00"},
		{"2021-04-30 21:14", "2021-04-30T21:14:00+00:00"},
		{"2021-04-30 21:14:10.052282", "2021-04-30T21:14:10+00:00"},
		{"2012-08-03 18:31:59.257000000", "2012-08-03T18:31:59+00:00"},
		{"2017-11-25 13:31:15 PST", "2017-11-25T21:31:15+00:00"},
		{"2014-12-16 06:20:00 UTC", "2014-12-16T06:20:00+00:00"},
		{"2014-04-26 13:13:43 +0800", "2014-04-26T05:13:43+00:00"},
		{"2014-04-26 13:13:44 +09:00", "2014-04-26T04:13:44+00:00"},
		{"2012-08-03 18:31:59.257000000 +0000", "2012-08-03T18:31:59+00:00"},
		{"2021-02-21", "2021-02-21T00:00:00+00:00"},
		{"2020-07-20+08:00", "2020-07-19T16:00:00+00:00"},
		{"Oct 7, 1970", "1970-10-07T00:00:00+00:00"},
		{"Oct. 7, 1970", "1970-10-07T00:00:00+00:00"},
		{"oct. 7, 1970", "1970-10-07T00:00:00+00:00"},
		{"October 7, 1970", "1970-10-07T00:00:00+00:00"},
		{"12 Feb 2006, 19:17", "2006-02-12T19:17:00+00:00"},
		{"12 Feb 2006 19:17", "2006-02-12T19:17:00+00:00"},
		{"14 May 2019 19:11:40.164", "2019-05-14T19:11:40+00:00"},
		{"4/8/2014 22:05", "2014-04-08T22:05:00+00:00"},
		{"04/2/2014 03:00:51", "2014-04-02T03:00:51+00:00"},
		{"8/8/1965 12:00:00 AM", "1965-08-08T00:00:00+00:00"},
		{"3/31/2014", "2014-03-31T00:00:00+00:00"},
		{"08/21/71", "1971-08-21T00:00:00+00:00"},
		{"2014/4/8 22:05", "2014-04-08T22:05:00+00:00"},
		{"2014/03/31", "2014-03-31T00:00:00+00:00"},
		{"3.31.2014", "2014-03-31T00:00:00+00:00"},
		{"2014.03.30", "2014-03-30T00:00:00+00:00"},
		{"171113 14:14:20", "2017-11-13T14:14:20+00:00"},
		{"2014年04月08日", "2014-04-08T00:00:00+00:00"},
		{"2014年04月08日11时25分18秒", "2014-04-08T11:25:18+00:00"},
		{"2014年04月08日 11:25:18", "2014-04-08T11:25:18+00:00"},
		{"2014-04-26 5:24:37pm", "2014-04-26T17:24:37+00:00"},
	}
	for _, tc := range cases {
		got, ok := TryParse(tc.in)
		if !ok || format(got) != tc.want {
			t.Errorf("TryParse(%q) = %v, %v; want %s", tc.in, got, ok, tc.want)
		}
	}
}

func TestDateOnlyIsStable(t *testing.T) {
	first, _ := ParseTimestamp("2026-01-06")
	second, _ := ParseTimestamp("2026-01-06")
	if !first.Equal(second) {
		t.Error("date-only value changed between calls")
	}
}

func TestParseChinese(t *testing.T) {
	for _, in := range []string{
		"週二, 6 一月 2026 14:28:00 +0000",
		"週一, 15 三月 2026 10:00:00 +0800",
		"週五, 25 十二月 2026 23:59:59 +0000",
		"週日, 1 七月 2026 00:00:00 -0500",
	} {
		if _, ok := parseChinese(in); !ok {
			t.Errorf("parseChinese(%q) failed", in)
		}
	}
	for _, in := range []string{
		"6 一月",
		"週二, 6 invalid 2026 14:28:00 +0000",
		"32 一月 2026 14:28:00",
		"0 一月 2026 14:28:00",
		"30 二月 2026 14:28:00",
		"6 一月 2026 25:28:00",
		"6 一月 2026 14:28:00 +9999",
	} {
		if _, ok := parseChinese(in); ok {
			t.Errorf("parseChinese(%q) should fail", in)
		}
	}
}

func TestParseChineseOffset(t *testing.T) {
	got, _ := parseChinese("週二, 6 一月 2026 14:28:00 +0800")
	if format(got) != "2026-01-06T06:28:00+00:00" {
		t.Errorf("got %v", got)
	}
	// An unreadable offset leaves the clock reading as UTC.
	got, _ = parseChinese("週二, 6 一月 2026 14:28:00 zz")
	if format(got) != "2026-01-06T14:28:00+00:00" {
		t.Errorf("got %v", got)
	}
}

func TestParseOffset(t *testing.T) {
	cases := map[string]int{"+0000": 0, "+0800": 8 * 3600, "-0500": -5 * 3600}
	for in, want := range cases {
		if got, ok := parseOffset(in); !ok || got != want {
			t.Errorf("parseOffset(%q) = %d, %v", in, got, ok)
		}
	}
	for _, in := range []string{"+08", "", "X0800", "+08中", "+中0800"} {
		if _, ok := parseOffset(in); ok {
			t.Errorf("parseOffset(%q) should fail", in)
		}
	}
}

func TestNormalizeTimezoneFormat(t *testing.T) {
	cases := []struct{ in, want string }{
		{"Thu, 22 Jan 2026 15:09:47 +08:00", "Thu, 22 Jan 2026 15:09:47 +0800"},
		{"Mon, 01 Jan 2026 12:00:00 -05:30", "Mon, 01 Jan 2026 12:00:00 -0530"},
		{"Thu, 22 Jan 2026 15:09:47 +0800", "Thu, 22 Jan 2026 15:09:47 +0800"},
		{"Thu, 22 Jan 2026 15:09:47 +08:00  ", "Thu, 22 Jan 2026 15:09:47 +0800"},
		{"日期x", "日期x"},
		{"2026年1月6日", "2026年1月6日"},
		{"abc", "abc"},
		{"12:00", "12:00"},
		{"foobar", "foobar"},
	}
	for _, tc := range cases {
		if got := NormalizeTimezoneFormat(tc.in); got != tc.want {
			t.Errorf("NormalizeTimezoneFormat(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestFormatDisplay(t *testing.T) {
	zone := time.FixedZone("x", 8*3600)
	if got := FormatDisplay(time.Date(2024, 3, 5, 18, 20, 30, 0, zone)); got != "2024-03-05 10:20:30" {
		t.Errorf("got %s", got)
	}
}

// parsed is empty when ParseTimestamp must reject the input.
var rustTimestampVectors = []struct{ input, normalized, parsed string }{
	{"2024-03-05T10:20:30Z", "2024-03-05T10:20:30Z", "2024-03-05 10:20:30"},
	{"2024-03-05T10:20:30+08:00", "2024-03-05T10:20:30+0800", "2024-03-05 02:20:30"},
	{"2024-03-05 10:20:30", "2024-03-05 10:20:30", "2024-03-05 10:20:30"},
	{"Tue, 05 Mar 2024 10:20:30 GMT", "Tue, 05 Mar 2024 10:20:30 GMT", "2024-03-05 10:20:30"},
	{"Tue, 05 Mar 2024 10:20:30 +0800", "Tue, 05 Mar 2024 10:20:30 +0800", "2024-03-05 02:20:30"},
	{"5 Mar 2024", "5 Mar 2024", "2024-03-05 00:00:00"},
	{"2024-03-05", "2024-03-05", "2024-03-05 00:00:00"},
	{"週二, 05 三月 2024 10:20:30 +0800", "週二, 05 三月 2024 10:20:30 +0800", "2024-03-05 02:20:30"},
	{"05 三月 2024 10:20:30", "05 三月 2024 10:20:30", "2024-03-05 10:20:30"},
	{"not a date", "not a date", ""},
	{"", "", ""},
}

func TestRustVectors(t *testing.T) {
	for _, row := range rustTimestampVectors {
		if got := NormalizeTimezoneFormat(row.input); got != row.normalized {
			t.Errorf("Normalize(%q) = %q, want %q", row.input, got, row.normalized)
		}
		got, ok := ParseTimestamp(row.input)
		switch {
		case row.parsed == "" && ok:
			t.Errorf("ParseTimestamp(%q) = %v, want none", row.input, got)
		case row.parsed != "" && (!ok || FormatDisplay(got) != row.parsed):
			t.Errorf("ParseTimestamp(%q) = %v, %v; want %s", row.input, got, ok, row.parsed)
		}
	}
}
