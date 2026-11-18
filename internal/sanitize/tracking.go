package sanitize

import "strings"

// trackingDomains are host prefixes that indicate a tracker.
var trackingDomains = []string{"pixel.", "beacon.", "track.", "analytics."}

// trackingPaths are URL path fragments that indicate a tracker.
var trackingPaths = []string{"/pixel", "/beacon", "/track", "/1x1"}

// trackingParams are removed on an exact, case-insensitive match. Entries
// must be lowercase because names are lowercased before the comparison; the
// Rust list had `__readwiseLocation` in mixed case, so it never matched.
var trackingParams = map[string]bool{
	"fbclid": true, "gclid": true, "dclid": true, "gbraid": true, "wbraid": true,
	"gclsrc": true, "srsltid": true, "yclid": true, "ysclid": true, "twclid": true,
	"msclkid": true, "mc_cid": true, "mc_eid": true, "mc_tc": true, "_openstat": true,
	"fb_action_ids": true, "fb_action_types": true, "fb_ref": true, "fb_source": true,
	"fb_comment_id": true, "hmb_campaign": true, "hmb_medium": true, "hmb_source": true,
	"itm_campaign": true, "itm_medium": true, "itm_source": true, "campaign_id": true,
	"campaign_medium": true, "campaign_name": true, "campaign_source": true,
	"campaign_term": true, "campaign_content": true, "wickedid": true, "hsa_cam": true,
	"_hsenc": true, "__hssc": true, "__hstc": true, "__hsfp": true, "_hsmi": true,
	"hsctatracking": true, "rb_clickid": true, "oly_anon_id": true, "oly_enc_id": true,
	"vero_id": true, "vero_conv": true, "mkt_tok": true, "sc_cid": true, "_bhlid": true,
	"_branch_match_id": true, "_branch_referrer": true, "__readwiselocation": true,
	"ref": true,
}

var trackingParamPrefixes = []string{"utm_", "mtm_"}

func isTrackingParam(name string) bool {
	lower := strings.ToLower(name)
	if trackingParams[lower] {
		return true
	}
	for _, prefix := range trackingParamPrefixes {
		if strings.HasPrefix(lower, prefix) {
			return true
		}
	}
	return false
}

// isTrackingPixel decides from an img's raw width, height and src.
func isTrackingPixel(width, height, src *string) bool {
	return hasTrackingSize(width, height) || hasTrackingURL(src)
}

func hasTrackingSize(width, height *string) bool {
	switch {
	case width != nil && height != nil:
		return *width == "1" && *height == "1"
	case width != nil:
		return *width == "0"
	case height != nil:
		return *height == "0"
	}
	return false
}

func hasTrackingURL(src *string) bool {
	if src == nil {
		return false
	}
	lower := strings.ToLower(*src)
	for _, d := range trackingDomains {
		if strings.Contains(lower, d) {
			return true
		}
	}
	for _, p := range trackingPaths {
		if strings.Contains(lower, p) {
			return true
		}
	}
	return false
}

// StripTrackingParams removes tracking query parameters from an http(s) URL and
// returns anything else unchanged. It is used for entry links shown to the
// reader and handed to external services.
func StripTrackingParams(rawURL string) string {
	if stripped, ok := stripTrackingParamsFromURL(rawURL); ok {
		return stripped
	}
	return rawURL
}

// stripTrackingParamsFromURL reports false when nothing was removed or the URL
// is not an http(s) URL.
func stripTrackingParamsFromURL(href string) (string, bool) {
	if !strings.HasPrefix(href, "http://") && !strings.HasPrefix(href, "https://") {
		return "", false
	}
	u, ok := parseURL(href, nil)
	if !ok {
		return "", false
	}
	pairs := parseQueryPairs(u.query)
	kept := make([]queryPair, 0, len(pairs))
	for _, p := range pairs {
		if !isTrackingParam(p.name) {
			kept = append(kept, p)
		}
	}
	if len(kept) == len(pairs) {
		return "", false
	}
	u.query = serializeQueryPairs(kept)
	u.hasQuery = len(kept) > 0
	return u.String(), true
}

type queryPair struct{ name, value string }

// parseQueryPairs decodes application/x-www-form-urlencoded data like
// `Url::query_pairs`: empty segments are skipped, `+` is a space and invalid
// UTF-8 becomes U+FFFD.
func parseQueryPairs(query string) []queryPair {
	var pairs []queryPair
	for _, segment := range strings.Split(query, "&") {
		if segment == "" {
			continue
		}
		name, value, _ := strings.Cut(segment, "=")
		pairs = append(pairs, queryPair{formDecode(name), formDecode(value)})
	}
	return pairs
}

func formDecode(s string) string {
	s = strings.ReplaceAll(s, "+", " ")
	decoded := percentDecodeLenient(s)
	return strings.ToValidUTF8(decoded, "�")
}

// percentDecodeLenient leaves malformed escapes as they are.
func percentDecodeLenient(s string) string {
	if !strings.Contains(s, "%") {
		return s
	}
	out := make([]byte, 0, len(s))
	for i := 0; i < len(s); i++ {
		if s[i] == '%' && i+2 < len(s) && isHex(s[i+1]) && isHex(s[i+2]) {
			out = append(out, unhex(s[i+1])<<4|unhex(s[i+2]))
			i += 2
			continue
		}
		out = append(out, s[i])
	}
	return string(out)
}

func isHex(c byte) bool {
	return c >= '0' && c <= '9' || c|0x20 >= 'a' && c|0x20 <= 'f'
}

func unhex(c byte) byte {
	if c <= '9' {
		return c - '0'
	}
	return c | 0x20 - 'a' + 10
}

func serializeQueryPairs(pairs []queryPair) string {
	parts := make([]string, len(pairs))
	for i, p := range pairs {
		parts[i] = formEncode(p.name) + "=" + formEncode(p.value)
	}
	return strings.Join(parts, "&")
}

// formEncode is `form_urlencoded::byte_serialize`: alphanumerics and `*-._`
// pass, a space becomes `+`, everything else is percent-encoded.
func formEncode(s string) string {
	const upperHex = "0123456789ABCDEF"
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c >= '0' && c <= '9', c|0x20 >= 'a' && c|0x20 <= 'z', c == '*', c == '-', c == '.', c == '_':
			b.WriteByte(c)
		case c == ' ':
			b.WriteByte('+')
		default:
			b.WriteByte('%')
			b.WriteByte(upperHex[c>>4])
			b.WriteByte(upperHex[c&15])
		}
	}
	return b.String()
}
