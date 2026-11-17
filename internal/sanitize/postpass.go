package sanitize

import (
	"net/url"
	"strings"

	"github.com/henry40408/rdrs/internal/imageproxy"
)

// Context is what HTML needs besides the markup.
type Context struct {
	// Secret signs proxied image URLs.
	Secret []byte
	// BaseURL resolves relative image URLs; empty means there is no document
	// base, so a relative image is dropped.
	BaseURL string
	// Referrer, when set, is signed into every proxied image URL.
	Referrer string
	// ProxyBaseURL makes proxied URLs absolute; empty gives root-relative ones.
	ProxyBaseURL string
}

// postPass removes tracking pixels, strips tracking parameters, routes every
// image through the signed proxy and adds link privacy attributes.
type postPass struct {
	ctx  Context
	base *url.URL
}

func newPostPass(ctx Context) postPass {
	pp := postPass{ctx: ctx}
	if base, err := url.Parse(trimURLInput(ctx.BaseURL)); err == nil && ctx.BaseURL != "" {
		pp.base = base
	}
	return pp
}

func (pp postPass) rewrite(nodes []*node) []*node {
	out := nodes[:0]
	for _, n := range nodes {
		if n.isText() {
			out = append(out, n)
			continue
		}
		n.children = pp.rewrite(n.children)
		switch n.tag {
		case "img":
			if !pp.rewriteImage(n) {
				continue
			}
		case "a":
			pp.rewriteLink(n)
		}
		out = append(out, n)
	}
	return out
}

// rewriteImage reports false when the image must be removed.
func (pp postPass) rewriteImage(img *node) bool {
	width := rawAttrPtr(img, "width")
	height := rawAttrPtr(img, "height")
	src := rawAttrPtr(img, "src")
	if src != nil {
		cleaned := strings.ReplaceAll(*src, "&amp;", "&")
		src = &cleaned
	}
	if isTrackingPixel(width, height, src) {
		return false
	}
	if src == nil || strings.HasPrefix(*src, "data:") {
		return true
	}
	// Fail closed: an unresolvable src would 404 on our origin, or be fetched
	// from the author's host outside the proxy and leak the reader's IP.
	absolute, ok := pp.absoluteImageURL(*src)
	if !ok {
		return false
	}
	img.setVerbatim("src", pp.proxyURL(absolute))
	img.setVerbatim("loading", "lazy")
	img.setVerbatim("decoding", "async")
	img.setVerbatim("data-img-state", "loading")
	return true
}

func (pp postPass) absoluteImageURL(src string) (string, bool) {
	if strings.HasPrefix(src, "http://") || strings.HasPrefix(src, "https://") {
		return src, true
	}
	return joinURL(pp.base, src)
}

func (pp postPass) proxyURL(absolute string) string {
	if pp.ctx.Referrer != "" {
		return imageproxy.CreateProxyURLWithReferrer(pp.ctx.Secret, absolute, pp.ctx.Referrer, pp.ctx.ProxyBaseURL)
	}
	return imageproxy.CreateProxyURL(pp.ctx.Secret, absolute, pp.ctx.ProxyBaseURL)
}

func (pp postPass) rewriteLink(a *node) {
	raw, ok := a.rawAttr("href")
	if !ok {
		return
	}
	href := strings.ReplaceAll(raw, "&amp;", "&")
	if !strings.HasPrefix(href, "http://") && !strings.HasPrefix(href, "https://") {
		return
	}
	if stripped, changed := stripTrackingParamsFromURL(href); changed {
		a.setVerbatim("href", stripped)
	}
	a.setVerbatim("target", "_blank")
	a.setVerbatim("referrerpolicy", "no-referrer")
}

func rawAttrPtr(n *node, key string) *string {
	v, ok := n.rawAttr(key)
	if !ok {
		return nil
	}
	return &v
}
