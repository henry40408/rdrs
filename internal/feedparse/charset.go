package feedparse

import (
	"bytes"
	"io"
	"regexp"

	"golang.org/x/net/html/charset"
)

// Decode converts a feed body to UTF-8. The character set comes from the
// Content-Type header, else a byte order mark, else the XML declaration, and
// is UTF-8 when none says otherwise. The Rust build assumed UTF-8 and showed
// mojibake for GBK or Big5 feeds; this is a deliberate difference.
func Decode(body []byte, contentType string) ([]byte, error) {
	contentType = withDeclaredCharset(body, contentType)
	reader, err := charset.NewReader(bytes.NewReader(body), contentType)
	if err != nil {
		return nil, err
	}
	decoded, err := io.ReadAll(reader)
	return bytes.TrimPrefix(decoded, []byte("\xef\xbb\xbf")), err
}

var (
	xmlDeclaration = regexp.MustCompile(`^\s*<\?xml[^>]*?encoding\s*=\s*["']([^"']+)["']`)
	headerCharset  = regexp.MustCompile(`(?i)charset\s*=`)
)

// withDeclaredCharset adds the XML declaration's encoding to the content type
// when the header names none: charset.NewReader reads only the header, a byte
// order mark and HTML meta tags, never the XML declaration.
func withDeclaredCharset(body []byte, contentType string) string {
	if headerCharset.MatchString(contentType) {
		return contentType
	}
	match := xmlDeclaration.FindSubmatch(body[:min(len(body), 256)])
	if match == nil {
		return contentType
	}
	if contentType == "" {
		contentType = "text/xml"
	}
	return contentType + "; charset=" + string(match[1])
}
