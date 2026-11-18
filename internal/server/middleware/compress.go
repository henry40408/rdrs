package middleware

import (
	"bytes"
	"compress/gzip"
	"net/http"
	"strings"

	"github.com/andybalholm/brotli"
)

// minCompressSize is tower-http's default: smaller bodies are left alone.
const minCompressSize = 32

// maxCompressBody is the largest body buffered for compression; a larger one
// is sent uncompressed so memory stays bounded.
const maxCompressBody = 8 << 20

// Compress encodes responses with gzip or br according to Accept-Encoding,
// following tower-http's CompressionLayer: the highest q-value wins and br
// beats gzip on a tie; q=0 refuses an encoding; when nothing is acceptable
// the status becomes 406. It skips bodies under 32 bytes, images except SVG,
// SSE and gRPC, and responses already encoded or ranged.
func Compress(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		encoding, acceptable := chooseEncoding(r.Header.Values("Accept-Encoding"))
		if !acceptable {
			next.ServeHTTP(&notAcceptableWriter{ResponseWriter: w}, r)
			return
		}
		if encoding == "" {
			// tower-http varies on Accept-Encoding even when it sends identity.
			withHead(next, func(header http.Header, status int) {
				if isCompressible(status, header) {
					addVaryAcceptEncoding(header)
				}
			}).ServeHTTP(w, r)
			return
		}
		dw := &deferredWriter{ResponseWriter: w, shouldBuffer: isCompressible, maxBody: maxCompressBody, spill: true}
		next.ServeHTTP(dw, r)
		if !dw.held() {
			return
		}
		body := dw.body.Bytes()
		if len(body) < minCompressSize {
			w.WriteHeader(dw.status)
			_, _ = w.Write(body)
			return
		}
		compressed, err := encode(encoding, body)
		if err != nil {
			w.WriteHeader(http.StatusInternalServerError)
			return
		}
		header := w.Header()
		addVaryAcceptEncoding(header)
		header.Del("Accept-Ranges")
		header.Del("Content-Length")
		header.Set("Content-Encoding", encoding)
		w.WriteHeader(dw.status)
		_, _ = w.Write(compressed)
	})
}

// isCompressible decides from the head alone, before any body is held.
func isCompressible(_ int, header http.Header) bool {
	if header.Get("Content-Encoding") != "" || header.Get("Content-Range") != "" {
		return false
	}
	contentType := header.Get("Content-Type")
	switch {
	case strings.HasPrefix(contentType, "text/event-stream"):
		return false
	case strings.HasPrefix(contentType, "application/grpc") && !strings.HasPrefix(contentType, "application/grpc-web"):
		return false
	case strings.HasPrefix(contentType, "image/") && !strings.HasPrefix(contentType, "image/svg+xml"):
		return false
	}
	return true
}

func encode(encoding string, body []byte) ([]byte, error) {
	var out bytes.Buffer
	var writer interface {
		Write([]byte) (int, error)
		Close() error
	}
	if encoding == "br" {
		writer = brotli.NewWriterLevel(&out, brotli.DefaultCompression)
	} else {
		writer = gzip.NewWriter(&out)
	}
	if _, err := writer.Write(body); err != nil {
		return nil, err
	}
	if err := writer.Close(); err != nil {
		return nil, err
	}
	return out.Bytes(), nil
}

func addVaryAcceptEncoding(header http.Header) {
	for _, value := range header.Values("Vary") {
		if strings.Contains(strings.ToLower(value), "accept-encoding") {
			return
		}
	}
	header.Add("Vary", "Accept-Encoding")
}

// notAcceptableWriter turns the response into a 406, leaving the body alone.
type notAcceptableWriter struct {
	http.ResponseWriter
	wroteHead bool
}

func (w *notAcceptableWriter) WriteHeader(status int) {
	if status < 200 {
		w.ResponseWriter.WriteHeader(status)
		return
	}
	if w.wroteHead {
		return
	}
	w.wroteHead = true
	addVaryAcceptEncoding(w.Header())
	w.ResponseWriter.WriteHeader(http.StatusNotAcceptable)
}

func (w *notAcceptableWriter) Write(data []byte) (int, error) {
	w.WriteHeader(http.StatusOK)
	return w.ResponseWriter.Write(data)
}

func (w *notAcceptableWriter) Unwrap() http.ResponseWriter { return w.ResponseWriter }

type offer struct {
	coding string
	q      int // thousandths
}

// codingRank breaks q-value ties: later is preferred.
var codingRank = map[string]int{"identity": 0, "gzip": 1, "br": 2}

// chooseEncoding returns "gzip", "br" or "" for identity. acceptable is false
// when the client refuses every option.
func chooseEncoding(headerValues []string) (encoding string, acceptable bool) {
	offers, wildcard := parseOffers(headerValues)
	best, bestQ := "", 0
	consider := func(coding string, q int) {
		if q > 0 && (q > bestQ || q == bestQ && codingRank[coding] > codingRank[best]) {
			best, bestQ = coding, q
		}
	}
	if wildcard != nil {
		for coding := range codingRank {
			q := *wildcard
			if explicit, ok := firstOffer(offers, coding); ok {
				q = explicit
			}
			consider(coding, q)
		}
	} else {
		for _, o := range offers {
			consider(o.coding, o.q)
		}
	}
	if best == "gzip" || best == "br" {
		return best, true
	}
	if best == "identity" {
		return "", true
	}
	if wildcard != nil {
		return "", false
	}
	identityRefused := false
	for _, o := range offers {
		identityRefused = identityRefused || o.coding == "identity" && o.q == 0
	}
	return "", !identityRefused
}

func firstOffer(offers []offer, coding string) (int, bool) {
	for _, o := range offers {
		if o.coding == coding {
			return o.q, true
		}
	}
	return 0, false
}

// parseOffers lists the supported codings the client named, and the q-value of
// the first "*". An item with a malformed q-value is ignored.
func parseOffers(headerValues []string) (offers []offer, wildcard *int) {
	for _, value := range headerValues {
		for _, item := range strings.Split(value, ",") {
			coding, qText, hasQ := strings.Cut(item, ";")
			coding = strings.TrimSpace(coding)
			q := 1000
			if hasQ {
				parsed, ok := parseQValue(strings.TrimSpace(qText))
				if !ok {
					continue
				}
				q = parsed
			}
			if coding == "*" {
				if wildcard == nil {
					wildcard = &q
				}
				continue
			}
			if name, known := knownCoding(coding); known {
				offers = append(offers, offer{coding: name, q: q})
			}
		}
	}
	return offers, wildcard
}

func knownCoding(coding string) (string, bool) {
	switch strings.ToLower(coding) {
	case "gzip", "x-gzip":
		return "gzip", true
	case "br":
		return "br", true
	case "identity":
		return "identity", true
	}
	return "", false
}

// parseQValue reads "q=0", "q=1", "q=0.5" and so on: up to three decimals,
// never above 1.
func parseQValue(text string) (int, bool) {
	if len(text) < 3 || (text[0] != 'q' && text[0] != 'Q') || text[1] != '=' {
		return 0, false
	}
	number := text[2:]
	value := 0
	switch number[0] {
	case '0':
	case '1':
		value = 1000
	default:
		return 0, false
	}
	if len(number) == 1 {
		return value, true
	}
	if number[1] != '.' {
		return 0, false
	}
	factor := 100
	for _, digit := range number[2:] {
		if digit < '0' || digit > '9' || factor < 1 {
			return 0, false
		}
		value += factor * int(digit-'0')
		factor /= 10
	}
	return value, value <= 1000
}
