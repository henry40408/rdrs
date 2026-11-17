package auth

import (
	"fmt"

	"github.com/go-webauthn/webauthn/protocol"
	"github.com/go-webauthn/webauthn/protocol/webauthncose"
	"github.com/go-webauthn/webauthn/webauthn"

	"github.com/henry40408/rdrs/internal/config"
)

// NewWebAuthn builds the relying party from the RDRS_WEBAUTHN_RP_* settings.
func NewWebAuthn(cfg config.Config) (*webauthn.WebAuthn, error) {
	relyingParty, err := webauthn.New(&webauthn.Config{
		RPID:          cfg.WebAuthnRPID,
		RPDisplayName: cfg.WebAuthnRPName,
		RPOrigins:     []string{cfg.WebAuthnRPOrigin},
	})
	if err != nil {
		return nil, fmt.Errorf("create webauthn: %w", err)
	}
	return relyingParty, nil
}

// PasskeyUser is the account side of one ceremony. The handle is random per
// registration and never stored: sign-in finds the account by credential id.
type PasskeyUser struct {
	Handle      []byte
	Name        string
	Credentials []webauthn.Credential
}

func (u PasskeyUser) WebAuthnID() []byte                         { return u.Handle }
func (u PasskeyUser) WebAuthnName() string                       { return u.Name }
func (u PasskeyUser) WebAuthnDisplayName() string                { return u.Name }
func (u PasskeyUser) WebAuthnCredentials() []webauthn.Credential { return u.Credentials }

// RegistrationOptions require a discoverable credential and user verification,
// because sign-in is usernameless. Only ES256 and RS256 are offered, as in the
// Rust build.
func RegistrationOptions(exclude []protocol.CredentialDescriptor) []webauthn.RegistrationOption {
	required := true
	return []webauthn.RegistrationOption{
		webauthn.WithExclusions(exclude),
		webauthn.WithCredentialParameters([]protocol.CredentialParameter{
			{Type: protocol.PublicKeyCredentialType, Algorithm: webauthncose.AlgES256},
			{Type: protocol.PublicKeyCredentialType, Algorithm: webauthncose.AlgRS256},
		}),
		webauthn.WithAuthenticatorSelection(protocol.AuthenticatorSelection{
			RequireResidentKey: &required,
			ResidentKey:        protocol.ResidentKeyRequirementRequired,
			UserVerification:   protocol.VerificationRequired,
		}),
		webauthn.WithConveyancePreference(protocol.PreferNoAttestation),
	}
}

// LoginOptions name no credentials: listing them would hand every passkey id to
// anonymous callers.
func LoginOptions() []webauthn.LoginOption {
	return []webauthn.LoginOption{webauthn.WithUserVerification(protocol.VerificationRequired)}
}
