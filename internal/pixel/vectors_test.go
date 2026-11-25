package pixel

// Expected values captured from the original Rust server; they must not be
// recomputed from the Go code.

const rustOptIn = "2024-03-05T00:00:00+00:00"
const rustGIFHex = "47494638396101000100800000000000ffffff21f90401000000002c00000000010001000002024401003b"

// injectedPixel is the tag appended to non-blank content of an entry created
// at or after the opt-in instant, for user 7 and entry 42.
const injectedPixel = `<img src="/p/7-42-768ecc13f8e773d6549b150cc17f0851.gif" width="1" height="1" alt="" aria-hidden="true">`

// baseURL is omitted when the context has none.
var rustURLVectors = []struct {
	userID, entryID int64
	baseURL, url    string
}{
	{userID: 7, entryID: 42, url: "/p/7-42-768ecc13f8e773d6549b150cc17f0851.gif"},
	{userID: 7, entryID: 42, baseURL: "https://rdrs.example", url: "https://rdrs.example/p/7-42-768ecc13f8e773d6549b150cc17f0851.gif"},
	{userID: 7, entryID: 42, baseURL: "https://rdrs.example/", url: "https://rdrs.example/p/7-42-768ecc13f8e773d6549b150cc17f0851.gif"},
	{userID: 0, entryID: 0, url: "/p/0-0-df320f98af8be73ed57f519c8e9a109e.gif"},
	{userID: 0, entryID: 0, baseURL: "https://rdrs.example", url: "https://rdrs.example/p/0-0-df320f98af8be73ed57f519c8e9a109e.gif"},
	{userID: 0, entryID: 0, baseURL: "https://rdrs.example/", url: "https://rdrs.example/p/0-0-df320f98af8be73ed57f519c8e9a109e.gif"},
	{userID: 1, entryID: 9223372036854775807, url: "/p/1-9223372036854775807-c6b6b196259f4b2edeb0f3f8cd55fc23.gif"},
	{userID: 1, entryID: 9223372036854775807, baseURL: "https://rdrs.example", url: "https://rdrs.example/p/1-9223372036854775807-c6b6b196259f4b2edeb0f3f8cd55fc23.gif"},
	{userID: 1, entryID: 9223372036854775807, baseURL: "https://rdrs.example/", url: "https://rdrs.example/p/1-9223372036854775807-c6b6b196259f4b2edeb0f3f8cd55fc23.gif"},
	{userID: -1, entryID: -2, url: "/p/-1--2-7395ffb21788be4c63bf384817721752.gif"},
	{userID: -1, entryID: -2, baseURL: "https://rdrs.example", url: "https://rdrs.example/p/-1--2-7395ffb21788be4c63bf384817721752.gif"},
	{userID: -1, entryID: -2, baseURL: "https://rdrs.example/", url: "https://rdrs.example/p/-1--2-7395ffb21788be4c63bf384817721752.gif"},
}

var rustInjectVectors = []struct {
	enabled                 bool
	createdAt, html, output string
}{
	{false, "2024-03-04T23:59:59+00:00", "<p>Article</p>", "<p>Article</p>"},
	{false, "2024-03-04T23:59:59+00:00", "", ""},
	{false, "2024-03-04T23:59:59+00:00", "  \n ", "  \n "},
	{false, "2024-03-04T23:59:59+00:00", "<p>x</p>", "<p>x</p>"},
	{false, "2024-03-05T00:00:00+00:00", "<p>Article</p>", "<p>Article</p>"},
	{false, "2024-03-05T00:00:00+00:00", "", ""},
	{false, "2024-03-05T00:00:00+00:00", "  \n ", "  \n "},
	{false, "2024-03-05T00:00:00+00:00", "<p>x</p>", "<p>x</p>"},
	{false, "2024-03-05T00:00:01+00:00", "<p>Article</p>", "<p>Article</p>"},
	{false, "2024-03-05T00:00:01+00:00", "", ""},
	{false, "2024-03-05T00:00:01+00:00", "  \n ", "  \n "},
	{false, "2024-03-05T00:00:01+00:00", "<p>x</p>", "<p>x</p>"},
	{true, "2024-03-04T23:59:59+00:00", "<p>Article</p>", "<p>Article</p>"},
	{true, "2024-03-04T23:59:59+00:00", "", ""},
	{true, "2024-03-04T23:59:59+00:00", "  \n ", "  \n "},
	{true, "2024-03-04T23:59:59+00:00", "<p>x</p>", "<p>x</p>"},
	{true, "2024-03-05T00:00:00+00:00", "<p>Article</p>", "<p>Article</p>" + injectedPixel},
	{true, "2024-03-05T00:00:00+00:00", "", ""},
	{true, "2024-03-05T00:00:00+00:00", "  \n ", "  \n "},
	{true, "2024-03-05T00:00:00+00:00", "<p>x</p>", "<p>x</p>" + injectedPixel},
	{true, "2024-03-05T00:00:01+00:00", "<p>Article</p>", "<p>Article</p>" + injectedPixel},
	{true, "2024-03-05T00:00:01+00:00", "", ""},
	{true, "2024-03-05T00:00:01+00:00", "  \n ", "  \n "},
	{true, "2024-03-05T00:00:01+00:00", "<p>x</p>", "<p>x</p>" + injectedPixel},
}
