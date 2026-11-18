package auth

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"

	"github.com/go-webauthn/webauthn/protocol"
	"github.com/go-webauthn/webauthn/protocol/webauthncbor"
	"github.com/go-webauthn/webauthn/protocol/webauthncose"
	"github.com/go-webauthn/webauthn/webauthn"
)

// The passkey.public_key column holds the JSON that webauthn-rs wrote for its
// Passkey type. Existing rows must keep working, and new rows keep the same
// shape so that a rollback to the Rust build can still read them.

// ErrUnsupportedPasskey is a stored key this build cannot represent.
var ErrUnsupportedPasskey = errors.New("unsupported passkey key type")

type legacyPasskey struct {
	Cred legacyCredential `json:"cred"`
}

type legacyCredential struct {
	CredID             binaryData        `json:"cred_id"`
	Cred               legacyCOSEKey     `json:"cred"`
	Counter            uint32            `json:"counter"`
	Transports         []string          `json:"transports"`
	UserVerified       bool              `json:"user_verified"`
	BackupEligible     bool              `json:"backup_eligible"`
	BackupState        bool              `json:"backup_state"`
	RegistrationPolicy string            `json:"registration_policy"`
	Extensions         json.RawMessage   `json:"extensions"`
	Attestation        legacyAttestation `json:"attestation"`
	AttestationFormat  string            `json:"attestation_format"`
}

// legacyAttestation is only ever written: webauthn-rs puts certificate chains
// and metadata objects here, which reading does not need.
type legacyAttestation struct {
	Data     json.RawMessage `json:"data"`
	Metadata json.RawMessage `json:"metadata"`
}

type legacyCOSEKey struct {
	Type string        `json:"type_"`
	Key  legacyKeyType `json:"key"`
}

// legacyKeyType is serde's externally tagged enum: exactly one member is set.
type legacyKeyType struct {
	EC2 *legacyEC2Key `json:"EC_EC2,omitempty"`
	OKP *legacyOKPKey `json:"EC_OKP,omitempty"`
	RSA *legacyRSAKey `json:"RSA,omitempty"`
}

type legacyEC2Key struct {
	Curve string     `json:"curve"`
	X     binaryData `json:"x"`
	Y     binaryData `json:"y"`
}

type legacyOKPKey struct {
	Curve string     `json:"curve"`
	X     binaryData `json:"x"`
}

// legacyRSAKey keeps the exponent as the number array serde writes for [u8; 3].
type legacyRSAKey struct {
	N binaryData `json:"n"`
	E []int      `json:"e"`
}

var legacyAlgorithms = map[string]webauthncose.COSEAlgorithmIdentifier{
	"ES256": webauthncose.AlgES256, "ES384": webauthncose.AlgES384, "ES512": webauthncose.AlgES512,
	"RS256": webauthncose.AlgRS256, "RS384": webauthncose.AlgRS384, "RS512": webauthncose.AlgRS512,
	"PS256": webauthncose.AlgPS256, "PS384": webauthncose.AlgPS384, "PS512": webauthncose.AlgPS512,
	"EDDSA": webauthncose.AlgEdDSA,
}

// legacyAliases are the older names webauthn-rs still accepts.
var legacyAliases = map[string]string{"ECDSA_SHA256": "ES256", "ECDSA_SHA384": "ES384", "ECDSA_SHA512": "ES512"}

var legacyCurves = map[string]webauthncose.COSEEllipticCurve{
	"SECP256R1": webauthncose.P256, "SECP384R1": webauthncose.P384, "SECP521R1": webauthncose.P521,
	"ED25519": webauthncose.Ed25519,
}

// DecodeLegacyPasskey turns a stored webauthn-rs Passkey into a credential.
// The counter comes from its own column, which the sign-in path keeps current;
// the counter inside the JSON is written once and never updated.
func DecodeLegacyPasskey(blob []byte, counter uint32) (webauthn.Credential, error) {
	var stored legacyPasskey
	if err := json.Unmarshal(blob, &stored); err != nil {
		return webauthn.Credential{}, fmt.Errorf("decode passkey: %w", err)
	}
	cred := stored.Cred
	publicKey, err := cosePublicKey(cred.Cred)
	if err != nil {
		return webauthn.Credential{}, err
	}
	transports := make([]protocol.AuthenticatorTransport, 0, len(cred.Transports))
	for _, name := range cred.Transports {
		transports = append(transports, protocol.AuthenticatorTransport(name))
	}
	return webauthn.Credential{
		ID:        cred.CredID,
		PublicKey: publicKey,
		Transport: transports,
		Flags: webauthn.CredentialFlags{
			UserPresent: true, UserVerified: cred.UserVerified,
			BackupEligible: cred.BackupEligible, BackupState: cred.BackupState,
		},
		Authenticator: webauthn.Authenticator{SignCount: counter},
	}, nil
}

func cosePublicKey(key legacyCOSEKey) ([]byte, error) {
	name := key.Type
	if alias, ok := legacyAliases[name]; ok {
		name = alias
	}
	algorithm, ok := legacyAlgorithms[name]
	if !ok {
		return nil, fmt.Errorf("%w: algorithm %q", ErrUnsupportedPasskey, key.Type)
	}
	header := webauthncose.PublicKeyData{Algorithm: int64(algorithm)}
	switch {
	case key.Key.EC2 != nil:
		curve, ok := legacyCurves[key.Key.EC2.Curve]
		if !ok {
			return nil, fmt.Errorf("%w: curve %q", ErrUnsupportedPasskey, key.Key.EC2.Curve)
		}
		header.KeyType = int64(webauthncose.EllipticKey)
		return webauthncbor.Marshal(webauthncose.EC2PublicKeyData{
			PublicKeyData: header, Curve: int64(curve), XCoord: key.Key.EC2.X, YCoord: key.Key.EC2.Y,
		})
	case key.Key.OKP != nil:
		curve, ok := legacyCurves[key.Key.OKP.Curve]
		if !ok {
			return nil, fmt.Errorf("%w: curve %q", ErrUnsupportedPasskey, key.Key.OKP.Curve)
		}
		header.KeyType = int64(webauthncose.OctetKey)
		return webauthncbor.Marshal(webauthncose.OKPPublicKeyData{
			PublicKeyData: header, Curve: int64(curve), XCoord: key.Key.OKP.X,
		})
	case key.Key.RSA != nil:
		exponent, err := rsaExponentBytes(key.Key.RSA.E)
		if err != nil {
			return nil, err
		}
		header.KeyType = int64(webauthncose.RSAKey)
		return webauthncbor.Marshal(webauthncose.RSAPublicKeyData{
			PublicKeyData: header, Modulus: key.Key.RSA.N, Exponent: exponent,
		})
	}
	return nil, fmt.Errorf("%w: no key", ErrUnsupportedPasskey)
}

func rsaExponentBytes(numbers []int) ([]byte, error) {
	exponent := make([]byte, len(numbers))
	for i, n := range numbers {
		if n < 0 || n > 255 {
			return nil, fmt.Errorf("%w: RSA exponent", ErrUnsupportedPasskey)
		}
		exponent[i] = byte(n)
	}
	return exponent, nil
}

// EncodeLegacyPasskey writes a credential in the webauthn-rs Passkey shape.
func EncodeLegacyPasskey(cred webauthn.Credential) ([]byte, error) {
	key, err := legacyKey(cred.PublicKey)
	if err != nil {
		return nil, err
	}
	transports := make([]string, 0, len(cred.Transport))
	for _, transport := range cred.Transport {
		transports = append(transports, legacyTransport(transport))
	}
	stored := legacyCredential{
		CredID: cred.ID, Cred: key, Counter: cred.Authenticator.SignCount,
		UserVerified: cred.Flags.UserVerified, BackupEligible: cred.Flags.BackupEligible,
		BackupState: cred.Flags.BackupState,
		// webauthn-rs registered every passkey under UV "required", and a
		// reader of this row needs the field.
		RegistrationPolicy: "required",
		Extensions:         json.RawMessage(`{}`),
		Attestation:        legacyAttestation{Data: json.RawMessage(`"None"`), Metadata: json.RawMessage(`"None"`)},
		AttestationFormat:  "none",
	}
	if len(transports) > 0 {
		stored.Transports = transports
	}
	return json.Marshal(legacyPasskey{Cred: stored})
}

func legacyTransport(transport protocol.AuthenticatorTransport) string {
	switch transport {
	case protocol.USB, protocol.NFC, protocol.BLE, protocol.Internal, protocol.Hybrid:
		return string(transport)
	}
	return "unknown"
}

func legacyKey(publicKey []byte) (legacyCOSEKey, error) {
	var header webauthncose.PublicKeyData
	if err := webauthncbor.Unmarshal(publicKey, &header); err != nil {
		return legacyCOSEKey{}, fmt.Errorf("decode public key: %w", err)
	}
	name, ok := legacyAlgorithmName(webauthncose.COSEAlgorithmIdentifier(header.Algorithm))
	if !ok {
		return legacyCOSEKey{}, fmt.Errorf("%w: algorithm %d", ErrUnsupportedPasskey, header.Algorithm)
	}
	var key legacyKeyType
	var err error
	switch webauthncose.COSEKeyType(header.KeyType) {
	case webauthncose.EllipticKey:
		key.EC2, err = legacyEC2(publicKey)
	case webauthncose.OctetKey:
		key.OKP, err = legacyOKP(publicKey)
	case webauthncose.RSAKey:
		key.RSA, err = legacyRSA(publicKey)
	default:
		err = fmt.Errorf("%w: key type %d", ErrUnsupportedPasskey, header.KeyType)
	}
	return legacyCOSEKey{Type: name, Key: key}, err
}

func legacyAlgorithmName(algorithm webauthncose.COSEAlgorithmIdentifier) (string, bool) {
	for name, candidate := range legacyAlgorithms {
		if candidate == algorithm {
			return name, true
		}
	}
	return "", false
}

func legacyCurveName(curve int64) (string, error) {
	for name, candidate := range legacyCurves {
		if int64(candidate) == curve {
			return name, nil
		}
	}
	return "", fmt.Errorf("%w: curve %d", ErrUnsupportedPasskey, curve)
}

func legacyEC2(publicKey []byte) (*legacyEC2Key, error) {
	var key webauthncose.EC2PublicKeyData
	if err := webauthncbor.Unmarshal(publicKey, &key); err != nil {
		return nil, err
	}
	curve, err := legacyCurveName(key.Curve)
	if err != nil {
		return nil, err
	}
	return &legacyEC2Key{Curve: curve, X: key.XCoord, Y: key.YCoord}, nil
}

func legacyOKP(publicKey []byte) (*legacyOKPKey, error) {
	var key webauthncose.OKPPublicKeyData
	if err := webauthncbor.Unmarshal(publicKey, &key); err != nil {
		return nil, err
	}
	curve, err := legacyCurveName(key.Curve)
	if err != nil {
		return nil, err
	}
	return &legacyOKPKey{Curve: curve, X: key.XCoord}, nil
}

func legacyRSA(publicKey []byte) (*legacyRSAKey, error) {
	var key webauthncose.RSAPublicKeyData
	if err := webauthncbor.Unmarshal(publicKey, &key); err != nil {
		return nil, err
	}
	// webauthn-rs stores the exponent as exactly three bytes.
	exponent := make([]int, 3)
	if len(key.Exponent) > 3 {
		return nil, fmt.Errorf("%w: RSA exponent", ErrUnsupportedPasskey)
	}
	for i, b := range key.Exponent {
		exponent[3-len(key.Exponent)+i] = int(b)
	}
	return &legacyRSAKey{N: key.Modulus, E: exponent}, nil
}

// binaryData is webauthn-rs' HumanBinaryData: written as unpadded URL-safe
// base64, read back from any of the four base64 variants or a byte array.
type binaryData []byte

func (d binaryData) MarshalJSON() ([]byte, error) {
	return json.Marshal(base64.RawURLEncoding.EncodeToString(d))
}

var binaryEncodings = []*base64.Encoding{
	base64.RawURLEncoding, base64.URLEncoding, base64.StdEncoding, base64.RawStdEncoding,
}

func (d *binaryData) UnmarshalJSON(data []byte) error {
	if bytes.HasPrefix(bytes.TrimSpace(data), []byte("[")) {
		var numbers []byte
		if err := json.Unmarshal(data, &numbers); err != nil {
			return err
		}
		*d = numbers
		return nil
	}
	var text string
	if err := json.Unmarshal(data, &text); err != nil {
		return err
	}
	for _, encoding := range binaryEncodings {
		if decoded, err := encoding.DecodeString(text); err == nil {
			*d = decoded
			return nil
		}
	}
	return errors.New("invalid base64 binary data")
}
