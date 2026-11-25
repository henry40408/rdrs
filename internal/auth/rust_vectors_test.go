package auth

import "strings"

// The vectors below were captured once from the Rust build during the Go
// rewrite. The original JSON files are in git history:
//
//	git show $(git log --diff-filter=D --format=%h -1 -- testdata/vectors/argon2_default.json)^:testdata/vectors/argon2_default.json
//
// (likewise argon2_fast.json and zxcvbn.json).

// argonHash is a hash the Rust build made for password. verifies is what Rust
// returned for password itself, wrongVerifies for password+"x".
type argonHash struct {
	hash          string
	password      string
	verifies      bool
	wrongVerifies bool
}

// argonVerifyCase is a hash, usually malformed, checked against an arbitrary
// password.
type argonVerifyCase struct {
	hash     string
	verifies bool
}

type argonVectors struct {
	hashed      []argonHash
	verifyCases []argonVerifyCase
}

// strengthVector is a password and the user inputs passed along with it.
// wantMessage is the Rust validation message, without its "Validation error: "
// prefix; "" means Rust accepted the password.
type strengthVector struct {
	password    string
	userInputs  []string
	wantMessage string
}

var argon2DefaultVectors = argonVectors{
	hashed: []argonHash{
		{
			hash:          "$argon2id$v=19$m=19456,t=2,p=1$becSjWSH0zjztbCLYLjHMw$NYx/3jC2fCaBqz4uCKbF+f9A7r5AwaraVc2Yxi2WnXI",
			password:      "correct horse battery staple",
			verifies:      true,
			wrongVerifies: false,
		},
		{
			hash:          "$argon2id$v=19$m=19456,t=2,p=1$CDWxIra63m4Rsu8py/DfVw$hXEIRoIUD6hW/Jc8lWalG3jlXGyF1PNua4HA+ebFgO0",
			password:      "日本語パスワード-15chars",
			verifies:      true,
			wrongVerifies: false,
		},
		{
			hash:          "$argon2id$v=19$m=19456,t=2,p=1$ruYfnkvK0u3eHPgX07ORSA$LOLyzUzIF2Eh8sD6ekXxzJbCfOAi1Crfr1JajpqYu1A",
			password:      "",
			verifies:      true,
			wrongVerifies: false,
		},
	},
	verifyCases: []argonVerifyCase{
		{hash: "!", verifies: false},
		{hash: "", verifies: false},
		{hash: "not-a-hash", verifies: false},
		{hash: "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$AAAA", verifies: false},
	},
}

var argon2FastVectors = argonVectors{
	hashed: []argonHash{
		{
			hash:          "$argon2id$v=19$m=8,t=1,p=1$x5JLwwauQimzk5cbeYbZxQ$lSQz5EG6ws82qDzdL6i7zPcQpMfL7ibVP4nu7F2zXqI",
			password:      "correct horse battery staple",
			verifies:      true,
			wrongVerifies: false,
		},
		{
			hash:          "$argon2id$v=19$m=8,t=1,p=1$Kl9hT4dWy5zQFteTxq6hEA$Zqw9X9jpZrAZg8xZO+npSzJ7Yz83Q2rbcoH1UlIaV0w",
			password:      "日本語パスワード-15chars",
			verifies:      true,
			wrongVerifies: false,
		},
		{
			hash:          "$argon2id$v=19$m=8,t=1,p=1$8vWoLlwJ4vpJRRdo/THIxA$GXzq3Rsd8VeROZv0RFx1f0/KhuSmv/OXShDrXv6Q5oU",
			password:      "",
			verifies:      true,
			wrongVerifies: false,
		},
	},
}

var strengthVectors = []strengthVector{
	{
		password:    "short",
		wantMessage: "Password must be at least 15 characters",
	},
	{
		password: "correct horse battery staple",
	},
	{
		password:    "passwordpassword",
		wantMessage: `Repeats like "abcabcabc" are only slightly harder to guess than "abc". Add another word or two. Uncommon words are better.`,
	},
	{
		password:    "aaaaaaaaaaaaaaaa",
		wantMessage: `Repeats like "aaa" are easy to guess. Add another word or two. Uncommon words are better.`,
	},
	{
		password:    "123456789012345",
		wantMessage: "Sequences like abc or 6543 are easy to guess. Add another word or two. Uncommon words are better.",
	},
	{
		password:    "qwertyuiopasdfgh",
		wantMessage: "This is similar to a commonly used password. Add another word or two. Uncommon words are better.",
	},
	{
		password:   "alice-the-admin-1",
		userInputs: []string{"alice"},
	},
	{
		password:    "alicealicealice",
		userInputs:  []string{"alice"},
		wantMessage: `Repeats like "abcabcabc" are only slightly harder to guess than "abc". Add another word or two. Uncommon words are better.`,
	},
	{
		password: "Tr0ub4dor&3-and-more",
	},
	{
		password:    "日本語日本語日本語日本語日本語",
		wantMessage: `Repeats like "abcabcabc" are only slightly harder to guess than "abc". Add another word or two. Uncommon words are better.`,
	},
	{
		password:    "日本語のパスワードです長い",
		wantMessage: "Password must be at least 15 characters",
	},
	{
		password:    "abcdefghijklmno",
		wantMessage: "Sequences like abc or 6543 are easy to guess. Add another word or two. Uncommon words are better.",
	},
	{
		password: "xkcd-battery-horse-staple-9",
	},
	{
		password:    "monkey-monkey-monkey",
		wantMessage: `Repeats like "abcabcabc" are only slightly harder to guess than "abc". Add another word or two. Uncommon words are better.`,
	},
	{
		password:    "letmeinletmeinletmein",
		wantMessage: `Repeats like "abcabcabc" are only slightly harder to guess than "abc". Add another word or two. Uncommon words are better.`,
	},
	{
		password: "Rr7!kQ2#vLp9@xZw4$",
	},
	{
		password:    strings.Repeat("a", 129),
		wantMessage: "Password must be at most 128 characters",
	},
}
