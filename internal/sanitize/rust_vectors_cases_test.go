package sanitize

// rustSanitizeSecret is the HMAC secret the Rust sanitize vectors were
// generated with. It keys the proxied image URLs in the expected outputs.
const rustSanitizeSecret = `test_secret_key_32_bytes_long!!!`

// rustSanitizeVector is one input with the Rust output for each context named
// in TestRustSanitizeVectors, and the Rust summary.
type rustSanitizeVector struct {
	input   string
	outputs map[string]string
	summary string
}

var rustSanitizeVectors = []rustSanitizeVector{
	{
		input: `<p>A <strong>bold</strong> and <em>italic</em> line.</p><ul><li>one</li></ul>`,
		outputs: map[string]string{
			"plain": `<p>A <strong>bold</strong> and <em>italic</em> line.</p><ul><li>one</li></ul>`,
			"base":  `<p>A <strong>bold</strong> and <em>italic</em> line.</p><ul><li>one</li></ul>`,
			"full":  `<p>A <strong>bold</strong> and <em>italic</em> line.</p><ul><li>one</li></ul>`,
		},
		summary: `<p>A <strong>bold</strong> and <em>italic</em> line.</p><ul><li>one</li></ul>`,
	},
	{
		input: `<style>@import url(//evil.tld/x.css);</style><p>Body</p><script>alert(1)</script>`,
		outputs: map[string]string{
			"plain": `<p>Body</p>`,
			"base":  `<p>Body</p>`,
			"full":  `<p>Body</p>`,
		},
		summary: `<p>Body</p>`,
	},
	{
		input: `<p onclick="x()">Body <img src="//evil.tld/beacon.gif"></p>`,
		outputs: map[string]string{
			"plain": `<p>Body </p>`,
			"base":  `<p>Body </p>`,
			"full":  `<p>Body </p>`,
		},
		summary: `<p>Body </p>`,
	},
	{
		input: `<p>See <a href="https://evil.tld/login">your account</a>.</p>`,
		outputs: map[string]string{
			"plain": `<p>See <a href="https://evil.tld/login" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">your account</a>.</p>`,
			"base":  `<p>See <a href="https://evil.tld/login" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">your account</a>.</p>`,
			"full":  `<p>See <a href="https://evil.tld/login" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">your account</a>.</p>`,
		},
		summary: `<p>See your account.</p>`,
	},
	{
		input: `**Bold** and _italic_ with a [link](https://example.com).`,
		outputs: map[string]string{
			"plain": `**Bold** and _italic_ with a [link](https://example.com).`,
			"base":  `**Bold** and _italic_ with a [link](https://example.com).`,
			"full":  `**Bold** and _italic_ with a [link](https://example.com).`,
		},
		summary: `**Bold** and _italic_ with a [link](https://example.com).`,
	},
	{
		input: `<p>Hello <strong>world</strong></p>`,
		outputs: map[string]string{
			"plain": `<p>Hello <strong>world</strong></p>`,
			"base":  `<p>Hello <strong>world</strong></p>`,
			"full":  `<p>Hello <strong>world</strong></p>`,
		},
		summary: `<p>Hello <strong>world</strong></p>`,
	},
	{
		input: `<p>Hello</p><script>alert('xss')</script>`,
		outputs: map[string]string{
			"plain": `<p>Hello</p>`,
			"base":  `<p>Hello</p>`,
			"full":  `<p>Hello</p>`,
		},
		summary: `<p>Hello</p>`,
	},
	{
		input: `<a href="javascript:alert('xss')">Click</a>`,
		outputs: map[string]string{
			"plain": `<a rel="noopener noreferrer">Click</a>`,
			"base":  `<a rel="noopener noreferrer">Click</a>`,
			"full":  `<a rel="noopener noreferrer">Click</a>`,
		},
		summary: `Click`,
	},
	{
		input: `<img src="https://example.com/image.jpg" alt="Image">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZS5qcGc&s=IHC0qf2vBDs" alt="Image" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZS5qcGc&s=IHC0qf2vBDs" alt="Image" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZS5qcGc&s=QvNpzYZcn08&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" alt="Image" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="data:image/png;base64,abc123" alt="Data URL">`,
		outputs: map[string]string{
			"plain": `<img alt="Data URL">`,
			"base":  `<img alt="Data URL">`,
			"full":  `<img alt="Data URL">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://a.com/1.jpg"><img src="https://b.com/2.jpg">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9hLmNvbS8xLmpwZw&s=FG4BeF_JNnw" loading="lazy" decoding="async" data-img-state="loading"><img src="/api/proxy/image?url=aHR0cHM6Ly9iLmNvbS8yLmpwZw&s=TS3lyqO3EoU" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9hLmNvbS8xLmpwZw&s=FG4BeF_JNnw" loading="lazy" decoding="async" data-img-state="loading"><img src="/api/proxy/image?url=aHR0cHM6Ly9iLmNvbS8yLmpwZw&s=TS3lyqO3EoU" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9hLmNvbS8xLmpwZw&s=sDKcOy9x1tU&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading"><img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9iLmNvbS8yLmpwZw&s=6R-Fwj6tBmI&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="data:image/svg+xml,%3Csvg%3E%3C/svg%3E" data-lazy-src="https://example.com/real.jpg" alt="Photo">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9yZWFsLmpwZw&s=LxJZZIYP7Zo" alt="Photo" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9yZWFsLmpwZw&s=LxJZZIYP7Zo" alt="Photo" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9yZWFsLmpwZw&s=NJxSVVMkUpc&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" alt="Photo" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="data:image/gif;base64,R0lGOD" data-src="https://example.com/photo.png">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5wbmc&s=icr4DShdLLY" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5wbmc&s=icr4DShdLLY" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5wbmc&s=bqev10MmfRY&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="data:image/svg+xml,%3Csvg%3E%3C/svg%3E" data-src="/img/pic.jpg">`,
		outputs: map[string]string{
			"plain": "",
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWcvcGljLmpwZw&s=ge2ll7Eo2uk" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWcvcGljLmpwZw&s=cfVyZJG4e_I&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://example.com/real.jpg" data-src="https://example.com/other.jpg">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9yZWFsLmpwZw&s=LxJZZIYP7Zo" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9yZWFsLmpwZw&s=LxJZZIYP7Zo" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9yZWFsLmpwZw&s=NJxSVVMkUpc&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<a href="https://example.com">Link</a>`,
		outputs: map[string]string{
			"plain": `<a href="https://example.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
			"base":  `<a href="https://example.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
			"full":  `<a href="https://example.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
		},
		summary: `Link`,
	},
	{
		input: `<a href="https://a.com">A</a><a href="https://b.com">B</a>`,
		outputs: map[string]string{
			"plain": `<a href="https://a.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">A</a><a href="https://b.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">B</a>`,
			"base":  `<a href="https://a.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">A</a><a href="https://b.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">B</a>`,
			"full":  `<a href="https://a.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">A</a><a href="https://b.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">B</a>`,
		},
		summary: `AB`,
	},
	{
		input: `<a href="/local/path">Local</a>`,
		outputs: map[string]string{
			"plain": `<a href="/local/path" rel="noopener noreferrer">Local</a>`,
			"base":  `<a href="/local/path" rel="noopener noreferrer">Local</a>`,
			"full":  `<a href="/local/path" rel="noopener noreferrer">Local</a>`,
		},
		summary: `Local`,
	},
	{
		input: `<p>Text</p><img data-src="https://example.com/real.jpg" src="https://pixel.tracker.com/p.gif" width="1" height="1">`,
		outputs: map[string]string{
			"plain": `<p>Text</p>`,
			"base":  `<p>Text</p>`,
			"full":  `<p>Text</p>`,
		},
		summary: `<p>Text</p>`,
	},
	{
		input: `<img data-src="https://pixel.tracker.com/x.gif" src="https://example.com/photo.jpg" width="800" height="600">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5qcGc&s=TAVEw-bfULw" width="800" height="600" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5qcGc&s=TAVEw-bfULw" width="800" height="600" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5qcGc&s=RpD8LhX6ql4&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" width="800" height="600" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://pixel.a.com/1.gif" width="1" height="1"><p>a</p><img src="https://beacon.b.com/2.gif"><p>b</p><img src="https://example.com/keep.jpg" width="800">`,
		outputs: map[string]string{
			"plain": `<p>a</p><p>b</p><img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9rZWVwLmpwZw&s=dz-yejk-Lxk" width="800" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<p>a</p><p>b</p><img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9rZWVwLmpwZw&s=dz-yejk-Lxk" width="800" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<p>a</p><p>b</p><img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9rZWVwLmpwZw&s=TbRXA7Bj3n4&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" width="800" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: `<p>a</p><p>b</p>`,
	},
	{
		input: `<img src="https://example.com/photo.jpg" width="800" height="600">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5qcGc&s=TAVEw-bfULw" width="800" height="600" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5qcGc&s=TAVEw-bfULw" width="800" height="600" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5qcGc&s=RpD8LhX6ql4&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" width="800" height="600" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<a href="https://example.com/search?q=rust&page=2">Link</a>`,
		outputs: map[string]string{
			"plain": `<a href="https://example.com/search?q=rust&amp;page=2" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
			"base":  `<a href="https://example.com/search?q=rust&amp;page=2" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
			"full":  `<a href="https://example.com/search?q=rust&amp;page=2" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
		},
		summary: `Link`,
	},
	{
		input: `<a href="https://example.com/page?id=123&fbclid=FB1&gclid=GC1&utm_source=test&valid=yes">Link</a>`,
		outputs: map[string]string{
			"plain": `<a href="https://example.com/page?id=123&valid=yes" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
			"base":  `<a href="https://example.com/page?id=123&valid=yes" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
			"full":  `<a href="https://example.com/page?id=123&valid=yes" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
		},
		summary: `Link`,
	},
	{
		input: `<a href="https://example.com/page">Link</a>`,
		outputs: map[string]string{
			"plain": `<a href="https://example.com/page" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
			"base":  `<a href="https://example.com/page" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
			"full":  `<a href="https://example.com/page" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">Link</a>`,
		},
		summary: `Link`,
	},
	{
		input: `<p>Text</p><img src="/images/photo.jpg" alt="Photo">`,
		outputs: map[string]string{
			"plain": `<p>Text</p>`,
			"base":  `<p>Text</p><img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZXMvcGhvdG8uanBn&s=K_4DTSA4Xp4" alt="Photo" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<p>Text</p><img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZXMvcGhvdG8uanBn&s=zLVswKleowg&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" alt="Photo" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: `<p>Text</p>`,
	},
	{
		input: `<img src="//evil.tld/x.gif">`,
		outputs: map[string]string{
			"plain": "",
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9ldmlsLnRsZC94LmdpZg&s=TTg9EctzuVE" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9ldmlsLnRsZC94LmdpZg&s=UeTYAZn_XKI&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://cdn.example.com/abs.jpg"><img src="/images/rel.jpg">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9jZG4uZXhhbXBsZS5jb20vYWJzLmpwZw&s=fvRZqYFN7jc" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9jZG4uZXhhbXBsZS5jb20vYWJzLmpwZw&s=fvRZqYFN7jc" loading="lazy" decoding="async" data-img-state="loading"><img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZXMvcmVsLmpwZw&s=vwU3rJnFvhA" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9jZG4uZXhhbXBsZS5jb20vYWJzLmpwZw&s=6GpIys1QHqM&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading"><img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZXMvcmVsLmpwZw&s=FK44wHsvtkQ&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://example.com/image.jpg">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZS5qcGc&s=IHC0qf2vBDs" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZS5qcGc&s=IHC0qf2vBDs" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9pbWFnZS5qcGc&s=QvNpzYZcn08&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<p>keep</p><img src="https://pixel.tracker.com/p.gif?q=a>b" width="1" height="1"><p>tail</p>`,
		outputs: map[string]string{
			"plain": `<p>keep</p><p>tail</p>`,
			"base":  `<p>keep</p><p>tail</p>`,
			"full":  `<p>keep</p><p>tail</p>`,
		},
		summary: `<p>keep</p><p>tail</p>`,
	},
	{
		input: `<img src="https://cdn.example.com/photo.jpg">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9jZG4uZXhhbXBsZS5jb20vcGhvdG8uanBn&s=rOA92sl9gdU" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9jZG4uZXhhbXBsZS5jb20vcGhvdG8uanBn&s=rOA92sl9gdU" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9jZG4uZXhhbXBsZS5jb20vcGhvdG8uanBn&s=qJzgrF8Gm38&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://example.com/a.jpg" width="640" height="480" alt="x">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLmpwZw&s=dunNZPRxfs0" width="640" height="480" alt="x" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLmpwZw&s=dunNZPRxfs0" width="640" height="480" alt="x" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLmpwZw&s=NQBA1c65HgA&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" width="640" height="480" alt="x" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://e.com/a.jpg" width="100" height="50" data-original-width="800" data-original-height="600">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9lLmNvbS9hLmpwZw&s=ZOTLl2bb420" width="100" height="50" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9lLmNvbS9hLmpwZw&s=ZOTLl2bb420" width="100" height="50" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9lLmNvbS9hLmpwZw&s=aI2DRPFnIdY&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" width="100" height="50" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://e.com/a.jpg" style="width:0px;height:0px">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9lLmNvbS9hLmpwZw&s=ZOTLl2bb420" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9lLmNvbS9hLmpwZw&s=ZOTLl2bb420" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9lLmNvbS9hLmpwZw&s=aI2DRPFnIdY&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://e.com/a.jpg" alt="x">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9lLmNvbS9hLmpwZw&s=ZOTLl2bb420" alt="x" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9lLmNvbS9hLmpwZw&s=ZOTLl2bb420" alt="x" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9lLmNvbS9hLmpwZw&s=aI2DRPFnIdY&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" alt="x" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<p aria-hidden="false">keep me</p><p>and me</p>`,
		outputs: map[string]string{
			"plain": `<p>keep me</p><p>and me</p>`,
			"base":  `<p>keep me</p><p>and me</p>`,
			"full":  `<p>keep me</p><p>and me</p>`,
		},
		summary: `<p>keep me</p><p>and me</p>`,
	},
	{
		input: `<p>body</p><span aria-hidden="TRUE">decor</span>`,
		outputs: map[string]string{
			"plain": `<p>body</p>`,
			"base":  `<p>body</p>`,
			"full":  `<p>body</p>`,
		},
		summary: `<p>body</p>decor`,
	},
	{
		input: `<div aria-hidden="true"><p>the entire article</p></div>`,
		outputs: map[string]string{
			"plain": `<div><p>the entire article</p></div>`,
			"base":  `<div><p>the entire article</p></div>`,
			"full":  `<div><p>the entire article</p></div>`,
		},
		summary: `<p>the entire article</p>`,
	},
	{
		input: `<p>body</p><span ARIA-HIDDEN="true">decor</span>`,
		outputs: map[string]string{
			"plain": `<p>body</p>`,
			"base":  `<p>body</p>`,
			"full":  `<p>body</p>`,
		},
		summary: `<p>body</p>decor`,
	},
	{
		input: `<img src="data:image/gif;base64,R0lGOD" DATA-SRC="https://example.com/photo.png">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5wbmc&s=icr4DShdLLY" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5wbmc&s=icr4DShdLLY" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9waG90by5wbmc&s=bqev10MmfRY&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://example.com/a.jpg" DATA-ORIGINAL-WIDTH="800" DATA-ORIGINAL-HEIGHT="600">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLmpwZw&s=dunNZPRxfs0" width="800" height="600" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLmpwZw&s=dunNZPRxfs0" width="800" height="600" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLmpwZw&s=NQBA1c65HgA&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" width="800" height="600" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<p>Plain <strong>body</strong> text.</p>`,
		outputs: map[string]string{
			"plain": `<p>Plain <strong>body</strong> text.</p>`,
			"base":  `<p>Plain <strong>body</strong> text.</p>`,
			"full":  `<p>Plain <strong>body</strong> text.</p>`,
		},
		summary: `<p>Plain <strong>body</strong> text.</p>`,
	},
	{
		input: "",
		outputs: map[string]string{
			"plain": "",
			"base":  "",
			"full":  "",
		},
		summary: "",
	},
	{
		input: `plain text & <angle> "quotes" 'apos'`,
		outputs: map[string]string{
			"plain": `plain text &amp;  "quotes" 'apos'`,
			"base":  `plain text &amp;  "quotes" 'apos'`,
			"full":  `plain text &amp;  "quotes" 'apos'`,
		},
		summary: `plain text &amp;  "quotes" 'apos'`,
	},
	{
		input: "<p>nbsp\u00a0here &nbsp; and &copy; &#169; &#xA9;</p>",
		outputs: map[string]string{
			"plain": "<p>nbsp&nbsp;here &nbsp; and \u00a9 \u00a9 \u00a9</p>",
			"base":  "<p>nbsp&nbsp;here &nbsp; and \u00a9 \u00a9 \u00a9</p>",
			"full":  "<p>nbsp&nbsp;here &nbsp; and \u00a9 \u00a9 \u00a9</p>",
		},
		summary: "<p>nbsp&nbsp;here &nbsp; and \u00a9 \u00a9 \u00a9</p>",
	},
	{
		input: `<p>unclosed <b>bold <i>italic`,
		outputs: map[string]string{
			"plain": `<p>unclosed <b>bold <i>italic</i></b></p>`,
			"base":  `<p>unclosed <b>bold <i>italic</i></b></p>`,
			"full":  `<p>unclosed <b>bold <i>italic</i></b></p>`,
		},
		summary: `<p>unclosed <b>bold <i>italic</i></b></p>`,
	},
	{
		input: `<table><tr><td>cell</td></tr></table>`,
		outputs: map[string]string{
			"plain": `<table><tbody><tr><td>cell</td></tr></tbody></table>`,
			"base":  `<table><tbody><tr><td>cell</td></tr></tbody></table>`,
			"full":  `<table><tbody><tr><td>cell</td></tr></tbody></table>`,
		},
		summary: `cell`,
	},
	{
		input: `<div><span lang="fr" title="t" class="c" id="i" style="color:red">x</span></div>`,
		outputs: map[string]string{
			"plain": `<div><span lang="fr" title="t">x</span></div>`,
			"base":  `<div><span lang="fr" title="t">x</span></div>`,
			"full":  `<div><span lang="fr" title="t">x</span></div>`,
		},
		summary: `x`,
	},
	{
		input: `<a href="HTTPS://Example.com/Path?utm_source=a&amp;id=2#f">link</a>`,
		outputs: map[string]string{
			"plain": `<a href="HTTPS://Example.com/Path?utm_source=a&amp;id=2#f" rel="noopener noreferrer">link</a>`,
			"base":  `<a href="HTTPS://Example.com/Path?utm_source=a&amp;id=2#f" rel="noopener noreferrer">link</a>`,
			"full":  `<a href="HTTPS://Example.com/Path?utm_source=a&amp;id=2#f" rel="noopener noreferrer">link</a>`,
		},
		summary: `link`,
	},
	{
		input: `<a href="mailto:a@b.c">mail</a><a href="ftp://x/y">ftp</a><a href="//cdn.example/x">proto</a>`,
		outputs: map[string]string{
			"plain": `<a rel="noopener noreferrer">mail</a><a rel="noopener noreferrer">ftp</a><a href="//cdn.example/x" rel="noopener noreferrer">proto</a>`,
			"base":  `<a rel="noopener noreferrer">mail</a><a rel="noopener noreferrer">ftp</a><a href="//cdn.example/x" rel="noopener noreferrer">proto</a>`,
			"full":  `<a rel="noopener noreferrer">mail</a><a rel="noopener noreferrer">ftp</a><a href="//cdn.example/x" rel="noopener noreferrer">proto</a>`,
		},
		summary: `mailftpproto`,
	},
	{
		input: `<img src="/rel.png" alt="a" title="t" width="100" height="50">`,
		outputs: map[string]string{
			"plain": "",
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9yZWwucG5n&s=-O9czd1gc_I" alt="a" title="t" width="100" height="50" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9yZWwucG5n&s=harIthceEAk&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" alt="a" title="t" width="100" height="50" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="https://example.com/a.png" srcset="a.png 1x" sizes="10px" loading="eager">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZw&s=GffI95pIygk" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZw&s=GffI95pIygk" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZw&s=Tc-0PV7_V6Y&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<p aria-hidden="true">hidden</p><p>shown</p>`,
		outputs: map[string]string{
			"plain": `<p>shown</p>`,
			"base":  `<p>shown</p>`,
			"full":  `<p>shown</p>`,
		},
		summary: `<p>hidden</p><p>shown</p>`,
	},
	{
		input: `<div aria-hidden="true"><p>all hidden</p></div>`,
		outputs: map[string]string{
			"plain": `<div><p>all hidden</p></div>`,
			"base":  `<div><p>all hidden</p></div>`,
			"full":  `<div><p>all hidden</p></div>`,
		},
		summary: `<p>all hidden</p>`,
	},
	{
		input: `<img data-src="https://example.com/l.png" width="1" height="1">`,
		outputs: map[string]string{
			"plain": "",
			"base":  "",
			"full":  "",
		},
		summary: "",
	},
	{
		input: `<img style="width:1px;height:1px" src="https://example.com/x.png">`,
		outputs: map[string]string{
			"plain": "",
			"base":  "",
			"full":  "",
		},
		summary: "",
	},
	{
		input: `<img src="https://pixel.example.com/x.gif"><img src="https://example.com/track/x.gif">`,
		outputs: map[string]string{
			"plain": "",
			"base":  "",
			"full":  "",
		},
		summary: "",
	},
	{
		input: `<blockquote cite="https://example.com/q">q</blockquote><blockquote cite="javascript:x">q</blockquote>`,
		outputs: map[string]string{
			"plain": `<blockquote cite="https://example.com/q">q</blockquote><blockquote cite="javascript:x">q</blockquote>`,
			"base":  `<blockquote cite="https://example.com/q">q</blockquote><blockquote cite="javascript:x">q</blockquote>`,
			"full":  `<blockquote cite="https://example.com/q">q</blockquote><blockquote cite="javascript:x">q</blockquote>`,
		},
		summary: `qq`,
	},
	{
		input: "<pre><code>a &lt; b\n  indented</code></pre>",
		outputs: map[string]string{
			"plain": "<pre><code>a &lt; b\n  indented</code></pre>",
			"base":  "<pre><code>a &lt; b\n  indented</code></pre>",
			"full":  "<pre><code>a &lt; b\n  indented</code></pre>",
		},
		summary: "<code>a &lt; b\n  indented</code>",
	},
	{
		input: `<ul><li>a<li>b</ul><ol start="3" type="a"><li>c</ol>`,
		outputs: map[string]string{
			"plain": `<ul><li>a</li><li>b</li></ul><ol start="3"><li>c</li></ol>`,
			"base":  `<ul><li>a</li><li>b</li></ul><ol start="3"><li>c</li></ol>`,
			"full":  `<ul><li>a</li><li>b</li></ul><ol start="3"><li>c</li></ol>`,
		},
		summary: `<ul><li>a</li><li>b</li></ul><ol start="3"><li>c</li></ol>`,
	},
	{
		input: `<h1>h</h1><h2>h</h2><h6>h</h6><hr><center>c</center><font color="red">f</font>`,
		outputs: map[string]string{
			"plain": `<h1>h</h1><h2>h</h2><h6>h</h6>cf`,
			"base":  `<h1>h</h1><h2>h</h2><h6>h</h6>cf`,
			"full":  `<h1>h</h1><h2>h</h2><h6>h</h6>cf`,
		},
		summary: `hhhcf`,
	},
	{
		input: `<figure><img src="https://example.com/f.png"><figcaption>cap</figcaption></figure>`,
		outputs: map[string]string{
			"plain": `<figure><img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9mLnBuZw&s=4FlvC7HJ1fk" loading="lazy" decoding="async" data-img-state="loading"><figcaption>cap</figcaption></figure>`,
			"base":  `<figure><img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9mLnBuZw&s=4FlvC7HJ1fk" loading="lazy" decoding="async" data-img-state="loading"><figcaption>cap</figcaption></figure>`,
			"full":  `<figure><img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9mLnBuZw&s=ylDnO5Y9oyw&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading"><figcaption>cap</figcaption></figure>`,
		},
		summary: `cap`,
	},
	{
		input: `<br/><br><p/>`,
		outputs: map[string]string{
			"plain": `<br><br><p></p>`,
			"base":  `<br><br><p></p>`,
			"full":  `<br><br><p></p>`,
		},
		summary: `<br><br><p></p>`,
	},
	{
		input: `<!-- comment --><p>after</p><![CDATA[cdata]]>`,
		outputs: map[string]string{
			"plain": `<p>after</p>`,
			"base":  `<p>after</p>`,
			"full":  `<p>after</p>`,
		},
		summary: `<p>after</p>`,
	},
	{
		input: `<svg><circle/></svg><math><mi>x</mi></math><iframe src="https://e/"></iframe><object data="x"></object>`,
		outputs: map[string]string{
			"plain": "",
			"base":  "",
			"full":  "",
		},
		summary: "",
	},
	{
		input: `<form action="/x"><input name="a"><button>b</button></form>`,
		outputs: map[string]string{
			"plain": `b`,
			"base":  `b`,
			"full":  `b`,
		},
		summary: `b`,
	},
	{
		input: `<p onclick="x()" onmouseover="y()">events</p>`,
		outputs: map[string]string{
			"plain": `<p>events</p>`,
			"base":  `<p>events</p>`,
			"full":  `<p>events</p>`,
		},
		summary: `<p>events</p>`,
	},
	{
		input: `<a href="https://example.com" target="_self" rel="nofollow" onclick="x">a</a>`,
		outputs: map[string]string{
			"plain": `<a href="https://example.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">a</a>`,
			"base":  `<a href="https://example.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">a</a>`,
			"full":  `<a href="https://example.com" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">a</a>`,
		},
		summary: `a`,
	},
	{
		input: `<td>orphan cell</td><tr><th>orphan row</th></tr>`,
		outputs: map[string]string{
			"plain": `orphan cellorphan row`,
			"base":  `orphan cellorphan row`,
			"full":  `orphan cellorphan row`,
		},
		summary: `orphan cellorphan row`,
	},
	{
		input: `<video src="v.mp4"><source src="s.mp4"></video><audio src="a.mp3"></audio>`,
		outputs: map[string]string{
			"plain": "",
			"base":  "",
			"full":  "",
		},
		summary: "",
	},
	{
		input: "<p>\u65e5\u672c\u8a9e\u3068<b>\u4e2d\u6587</b></p>",
		outputs: map[string]string{
			"plain": "<p>\u65e5\u672c\u8a9e\u3068<b>\u4e2d\u6587</b></p>",
			"base":  "<p>\u65e5\u672c\u8a9e\u3068<b>\u4e2d\u6587</b></p>",
			"full":  "<p>\u65e5\u672c\u8a9e\u3068<b>\u4e2d\u6587</b></p>",
		},
		summary: "<p>\u65e5\u672c\u8a9e\u3068<b>\u4e2d\u6587</b></p>",
	},
	{
		input: `<a href="https://example.com/?ref=x&amp;fbclid=y&amp;gclid=z&amp;keep=1">c</a>`,
		outputs: map[string]string{
			"plain": `<a href="https://example.com/?keep=1" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">c</a>`,
			"base":  `<a href="https://example.com/?keep=1" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">c</a>`,
			"full":  `<a href="https://example.com/?keep=1" rel="noopener noreferrer" target="_blank" referrerpolicy="no-referrer">c</a>`,
		},
		summary: `c`,
	},
	{
		input: `<img src="https://example.com/a.png?x=1&amp;y=2">`,
		outputs: map[string]string{
			"plain": `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZz94PTEmeT0y&s=riEXmF-nlp8" loading="lazy" decoding="async" data-img-state="loading">`,
			"base":  `<img src="/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZz94PTEmeT0y&s=riEXmF-nlp8" loading="lazy" decoding="async" data-img-state="loading">`,
			"full":  `<img src="https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZz94PTEmeT0y&s=l22XhdPfrQ8&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8" loading="lazy" decoding="async" data-img-state="loading">`,
		},
		summary: "",
	},
	{
		input: `<img src="data:image/png;base64,AAAA">`,
		outputs: map[string]string{
			"plain": `<img>`,
			"base":  `<img>`,
			"full":  `<img>`,
		},
		summary: "",
	},
}

var rustTrackingParamVectors = []struct{ url, stripped string }{
	{url: `https://example.com/a?utm_source=x&id=1`, stripped: `https://example.com/a?id=1`},
	{url: `https://example.com/a?fbclid=abc`, stripped: `https://example.com/a`},
	{url: `https://example.com/a#frag`, stripped: `https://example.com/a#frag`},
	{url: `https://example.com/a?b=2&a=1&utm_medium=m`, stripped: `https://example.com/a?b=2&a=1`},
	{url: `not a url`, stripped: `not a url`},
}
