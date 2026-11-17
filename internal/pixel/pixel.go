// Package pixel builds the open-tracking pixel appended to rendered entry
// content, so the server learns which entries a client rendered.
//
// Inject on the output of sanitize.HTML, never its input: the sanitiser
// strips 1x1 images and proxies every <img>, which would destroy a
// same-origin pixel.
package pixel

import (
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/secret"
)

// TransparentGIF is served for every pixel request, valid or not. It is
// inlined so the response depends on nothing the request controls.
var TransparentGIF = []byte{
	0x47, 0x49, 0x46, 0x38, 0x39, 0x61, // "GIF89a"
	0x01, 0x00, 0x01, 0x00, // 1x1
	0x80, 0x00, 0x00, // global colour table, 2 entries
	0x00, 0x00, 0x00, // colour 0: black (the transparent one)
	0xFF, 0xFF, 0xFF, // colour 1: white
	0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, // graphic control: index 0 transparent
	0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, // image descriptor
	0x02, 0x02, 0x44, 0x01, 0x00, // LZW image data
	0x3B, // trailer
}

// PathPrefix is short, outside /api, and ends in .gif so clients treat it as
// an image. The request carries no session and must not be given one.
const PathPrefix = "/p/"

// Context holds what is needed to decide on, address and sign a pixel. Build
// it once per request, not per entry.
type Context struct {
	UserID int64
	// EnabledAt is when this reader opted in; nil means opted out.
	EnabledAt *time.Time
	// Secret is the root key; the signature derives under secret.DomainPixel.
	Secret []byte
	// BaseURL makes the pixel absolute for content rendered off-origin;
	// empty yields a root-relative URL.
	BaseURL string
}

// Disabled is an opted-out context, for paths with no reader settings.
func Disabled(userID int64, rootSecret []byte) Context {
	return Context{UserID: userID, Secret: rootSecret}
}

// IsEnabled reports whether this reader tracks opens at all.
func (c Context) IsEnabled() bool { return c.EnabledAt != nil }

// URL is the pixel URL for one entry.
func (c Context) URL(entryID int64) string {
	sig := secret.PixelSig(c.Secret, c.UserID, entryID)
	path := PathPrefix + strconv.FormatInt(c.UserID, 10) + "-" +
		strconv.FormatInt(entryID, 10) + "-" + sig + ".gif"
	if c.BaseURL == "" {
		return path
	}
	return strings.TrimRight(c.BaseURL, "/") + path
}

func (c Context) imgTag(entryID int64) string {
	return `<img src="` + c.URL(entryID) + `" width="1" height="1" alt="" aria-hidden="true">`
}

// MaybeInject appends the pixel when tracking is on and the entry was created
// at or after the opt-in; older entries are outside the denominator.
func (c Context) MaybeInject(html string, entryID int64, entryCreatedAt time.Time) string {
	if c.EnabledAt == nil || entryCreatedAt.Before(*c.EnabledAt) {
		return html
	}
	// No article rendered, so no open to record.
	if strings.TrimSpace(html) == "" {
		return html
	}
	return html + c.imgTag(entryID)
}
