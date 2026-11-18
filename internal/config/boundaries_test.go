package config

import (
	"bytes"
	"encoding/base64"
	"testing"
)

func TestSecretOfExactlyTheMinimumLengthIsDecoded(t *testing.T) {
	want := bytes.Repeat([]byte{7}, MinSecretLen)
	encoded := base64.StdEncoding.EncodeToString(want)
	secret, generated := loadSecret(&encoded)
	if generated || !bytes.Equal(secret, want) {
		t.Errorf("secret = %v, generated = %v", secret, generated)
	}
}
