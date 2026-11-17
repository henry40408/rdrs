// Package sanitize turns untrusted feed markup into markup that is safe to
// render unescaped. It reproduces the Rust pipeline (pre-passes, an ammonia
// style allowlist, then a post-pass) on top of golang.org/x/net/html.
package sanitize

// HTML sanitises content to the allowlist, then removes tracking pixels and
// parameters, adds link privacy attributes and routes every image through the
// signed image proxy.
//
// An image src that cannot resolve to an absolute http(s) URL is dropped, so
// pass the entry's content base URL; an empty ctx.BaseURL is for callers with
// no document base, and tests.
func HTML(content string, ctx Context) string {
	root := prepare(content)
	cleaned := articlePolicy.clean(root)
	return render(newPostPass(ctx).rewrite(cleaned))
}

// Summary reduces a model-written summary to inline prose markup. A stray `<`
// is escaped rather than opening a tag.
func Summary(content string) string {
	return render(summaryPolicy.clean(parseFragment(content)))
}
