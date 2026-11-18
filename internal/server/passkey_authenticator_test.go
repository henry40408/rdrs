package server

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"testing"

	"github.com/go-webauthn/webauthn/protocol/webauthncbor"
	"github.com/go-webauthn/webauthn/protocol/webauthncose"
)

const (
	testRPID     = "localhost"
	testOrigin   = "http://localhost:8080"
	flagPresent  = 0x01
	flagVerified = 0x04
	flagAttested = 0x40
)

// softPasskey is a software authenticator: an ES256 key that answers
// registration with a "none" attestation and sign-in with a signed assertion.
type softPasskey struct {
	t            *testing.T
	key          *ecdsa.PrivateKey
	credentialID []byte
	userHandle   []byte
	counter      uint32
}

func newSoftPasskey(t *testing.T) *softPasskey {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	id := make([]byte, 32)
	if _, err := rand.Read(id); err != nil {
		t.Fatal(err)
	}
	return &softPasskey{t: t, key: key, credentialID: id}
}

func b64(data []byte) string { return base64.RawURLEncoding.EncodeToString(data) }

func (s *softPasskey) coseKey() []byte {
	point, err := s.key.PublicKey.Bytes() // 0x04 || X || Y
	if err != nil {
		s.t.Fatal(err)
	}
	encoded, err := webauthncbor.Marshal(webauthncose.EC2PublicKeyData{
		PublicKeyData: webauthncose.PublicKeyData{KeyType: int64(webauthncose.EllipticKey), Algorithm: int64(webauthncose.AlgES256)},
		Curve:         int64(webauthncose.P256),
		XCoord:        point[1:33],
		YCoord:        point[33:],
	})
	if err != nil {
		s.t.Fatal(err)
	}
	return encoded
}

func (s *softPasskey) authData(rpID string, flags byte, attested bool) []byte {
	hash := sha256.Sum256([]byte(rpID))
	data := append(hash[:], flags)
	data = binary.BigEndian.AppendUint32(data, s.counter)
	if !attested {
		return data
	}
	data = append(data, make([]byte, 16)...)
	data = binary.BigEndian.AppendUint16(data, uint16(len(s.credentialID)))
	data = append(data, s.credentialID...)
	return append(data, s.coseKey()...)
}

func clientData(kind, challenge, origin string) []byte {
	encoded, _ := json.Marshal(map[string]any{"type": kind, "challenge": challenge, "origin": origin, "crossOrigin": false})
	return encoded
}

// registration answers a creation request. The challenge is the base64url text
// the server sent.
func (s *softPasskey) registration(challenge string, flags byte) map[string]any {
	s.counter = 0
	attestation, err := webauthncbor.Marshal(map[string]any{
		"fmt": "none", "attStmt": map[string]any{}, "authData": s.authData(testRPID, flags|flagAttested, true),
	})
	if err != nil {
		s.t.Fatal(err)
	}
	return map[string]any{
		"id": b64(s.credentialID), "rawId": b64(s.credentialID), "type": "public-key",
		"response": map[string]any{
			"attestationObject": b64(attestation),
			"clientDataJSON":    b64(clientData("webauthn.create", challenge, testOrigin)),
			"transports":        []string{"internal"},
		},
	}
}

// assertion answers a request: it counts one more use and signs it.
func (s *softPasskey) assertion(challenge string, flags byte) map[string]any {
	s.counter++
	return s.signedAssertion(challenge, flags, testOrigin)
}

func (s *softPasskey) signedAssertion(challenge string, flags byte, origin string) map[string]any {
	authData := s.authData(testRPID, flags, false)
	client := clientData("webauthn.get", challenge, origin)
	clientHash := sha256.Sum256(client)
	digest := sha256.Sum256(append(append([]byte{}, authData...), clientHash[:]...))
	signature, err := ecdsa.SignASN1(rand.Reader, s.key, digest[:])
	if err != nil {
		s.t.Fatal(err)
	}
	return map[string]any{
		"id": b64(s.credentialID), "rawId": b64(s.credentialID), "type": "public-key",
		"response": map[string]any{
			"authenticatorData": b64(authData), "clientDataJSON": b64(client),
			"signature": b64(signature), "userHandle": b64(s.userHandle),
		},
	}
}

func decodeB64(t *testing.T, text string) []byte {
	t.Helper()
	decoded, err := base64.RawURLEncoding.DecodeString(text)
	if err != nil {
		t.Fatal(err)
	}
	return decoded
}
