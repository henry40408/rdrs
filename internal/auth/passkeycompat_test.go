package auth

import (
	"bytes"
	"errors"
	"strings"
	"testing"

	"github.com/go-webauthn/webauthn/protocol"
	"github.com/go-webauthn/webauthn/protocol/webauthncbor"
	"github.com/go-webauthn/webauthn/protocol/webauthncose"
)

// These documents are written the way webauthn-rs 0.5 serialises a Passkey.
const (
	legacyES256 = `{"cred":{"cred_id":"AQID","cred":{"type_":"ES256","key":{"EC_EC2":{"curve":"SECP256R1","x":"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8","y":"ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8"}}},"counter":7,"transports":["usb","internal"],"user_verified":true,"backup_eligible":true,"backup_state":false,"registration_policy":"required","extensions":{"cred_protect":"NotRequested"},"attestation":{"data":"None","metadata":"None"},"attestation_format":"none"}}`
	legacyRS256 = `{"cred":{"cred_id":"BAUG","cred":{"type_":"RS256","key":{"RSA":{"n":"qrvM","e":[1,0,1]}}},"counter":0,"transports":null,"user_verified":true,"backup_eligible":false,"backup_state":false,"registration_policy":"required","extensions":{},"attestation":{"data":"None","metadata":"None"},"attestation_format":"none"}}`
	legacyEdDSA = `{"cred":{"cred_id":"BwgJ","cred":{"type_":"EDDSA","key":{"EC_OKP":{"curve":"ED25519","x":"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"}}},"counter":1,"transports":[],"user_verified":false,"backup_eligible":false,"backup_state":false,"registration_policy":"required","extensions":{},"attestation":{"data":"None","metadata":"None"},"attestation_format":"none"}}`
)

func TestDecodeLegacyES256(t *testing.T) {
	cred, err := DecodeLegacyPasskey([]byte(legacyES256), 9)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(cred.ID, []byte{1, 2, 3}) {
		t.Errorf("id = %v", cred.ID)
	}
	// The counter column, not the document, is current.
	if cred.Authenticator.SignCount != 9 {
		t.Errorf("counter = %d", cred.Authenticator.SignCount)
	}
	flags := cred.Flags
	if !flags.UserPresent || !flags.UserVerified || !flags.BackupEligible || flags.BackupState {
		t.Errorf("flags = %+v", flags)
	}
	if len(cred.Transport) != 2 || cred.Transport[0] != protocol.USB || cred.Transport[1] != protocol.Internal {
		t.Errorf("transports = %v", cred.Transport)
	}
	var key webauthncose.EC2PublicKeyData
	if err := webauthncbor.Unmarshal(cred.PublicKey, &key); err != nil {
		t.Fatal(err)
	}
	if key.KeyType != int64(webauthncose.EllipticKey) || key.Algorithm != int64(webauthncose.AlgES256) ||
		key.Curve != int64(webauthncose.P256) || len(key.XCoord) != 32 || key.XCoord[31] != 0x1f || key.YCoord[0] != 0x20 {
		t.Errorf("key = %+v", key)
	}
}

func TestDecodeLegacyIgnoresStructuredExtensionsAndAttestation(t *testing.T) {
	document := strings.NewReplacer(
		`"extensions":{"cred_protect":"NotRequested"}`, `"extensions":{"cred_props":{"Set":{"rk":true}},"hmac_create_secret":{"Set":true}}`,
		`"attestation":{"data":"None","metadata":"None"}`, `"attestation":{"data":{"Basic":["AAAA"]},"metadata":{"Packed":{"aaguid":"00000000-0000-0000-0000-000000000000"}}}`,
	).Replace(legacyES256)
	if _, err := DecodeLegacyPasskey([]byte(document), 0); err != nil {
		t.Fatal(err)
	}
}

func TestDecodeLegacyRSAAndEdDSA(t *testing.T) {
	rsaCred, err := DecodeLegacyPasskey([]byte(legacyRS256), 0)
	if err != nil {
		t.Fatal(err)
	}
	var rsaKey webauthncose.RSAPublicKeyData
	if err := webauthncbor.Unmarshal(rsaCred.PublicKey, &rsaKey); err != nil {
		t.Fatal(err)
	}
	if rsaKey.Algorithm != int64(webauthncose.AlgRS256) || !bytes.Equal(rsaKey.Exponent, []byte{1, 0, 1}) ||
		!bytes.Equal(rsaKey.Modulus, []byte{0xaa, 0xbb, 0xcc}) || len(rsaCred.Transport) != 0 {
		t.Errorf("rsa = %+v", rsaKey)
	}
	edCred, err := DecodeLegacyPasskey([]byte(legacyEdDSA), 1)
	if err != nil {
		t.Fatal(err)
	}
	var edKey webauthncose.OKPPublicKeyData
	if err := webauthncbor.Unmarshal(edCred.PublicKey, &edKey); err != nil {
		t.Fatal(err)
	}
	if edKey.Algorithm != int64(webauthncose.AlgEdDSA) || edKey.Curve != int64(webauthncose.Ed25519) || len(edKey.XCoord) != 32 {
		t.Errorf("ed = %+v", edKey)
	}
}

func TestLegacyRoundTripKeepsTheDocumentShape(t *testing.T) {
	for _, document := range []string{legacyES256, legacyRS256, legacyEdDSA} {
		cred, err := DecodeLegacyPasskey([]byte(document), 3)
		if err != nil {
			t.Fatal(err)
		}
		encoded, err := EncodeLegacyPasskey(cred)
		if err != nil {
			t.Fatal(err)
		}
		again, err := DecodeLegacyPasskey(encoded, 3)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(again.PublicKey, cred.PublicKey) || !bytes.Equal(again.ID, cred.ID) || again.Flags != cred.Flags {
			t.Errorf("round trip changed %s", document[:40])
		}
		for _, field := range []string{`"cred_id"`, `"registration_policy":"required"`, `"attestation":{"data":"None","metadata":"None"}`, `"attestation_format":"none"`} {
			if !strings.Contains(string(encoded), field) {
				t.Errorf("encoded document lacks %s: %s", field, encoded)
			}
		}
	}
}

func TestLegacyBinaryAcceptsEveryBase64Variant(t *testing.T) {
	// 0xfb 0xff is "-_8" in URL-safe and "+/8=" in standard base64.
	for _, text := range []string{"-_8", "-_8=", "+/8=", "+/8"} {
		var data binaryData
		if err := data.UnmarshalJSON([]byte(`"` + text + `"`)); err != nil || !bytes.Equal(data, []byte{0xfb, 0xff}) {
			t.Errorf("%q = %v, %v", text, data, err)
		}
	}
	var fromArray binaryData
	if err := fromArray.UnmarshalJSON([]byte(`[1,2,3]`)); err != nil || !bytes.Equal(fromArray, []byte{1, 2, 3}) {
		t.Errorf("array = %v, %v", fromArray, err)
	}
	if err := fromArray.UnmarshalJSON([]byte(`"***"`)); err == nil {
		t.Error("garbage accepted")
	}
}

func TestEncodeLegacyNamesUnknownTransports(t *testing.T) {
	cred, _ := DecodeLegacyPasskey([]byte(legacyES256), 0)
	cred.Transport = []protocol.AuthenticatorTransport{"smart-card", protocol.Hybrid}
	encoded, err := EncodeLegacyPasskey(cred)
	if err != nil || !strings.Contains(string(encoded), `"transports":["unknown","hybrid"]`) {
		t.Errorf("%s %v", encoded, err)
	}
}

func TestDecodeLegacyRefusesWhatItCannotRepresent(t *testing.T) {
	for name, document := range map[string]string{
		"not json":          `nope`,
		"unknown algorithm": strings.Replace(legacyES256, `"ES256"`, `"INSECURE_RS1"`, 1),
		"unknown curve":     strings.Replace(legacyES256, `SECP256R1`, `SECP999`, 1),
		"ed448":             strings.Replace(legacyEdDSA, `ED25519`, `ED448`, 1),
		"no key":            `{"cred":{"cred_id":"AQ","cred":{"type_":"ES256","key":{}}}}`,
		"bad exponent":      strings.Replace(legacyRS256, `[1,0,1]`, `[1,0,300]`, 1),
	} {
		_, err := DecodeLegacyPasskey([]byte(document), 0)
		if err == nil {
			t.Errorf("%s accepted", name)
		}
		if name != "not json" && !errors.Is(err, ErrUnsupportedPasskey) {
			t.Errorf("%s: %v", name, err)
		}
	}
}
