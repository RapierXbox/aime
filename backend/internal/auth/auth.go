package auth

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"errors"

	"github.com/cloudflare/circl/sign/mldsa/mldsa65"
	"golang.org/x/crypto/argon2"
)

var (
	ErrInvalidKey       = errors.New("invalid key")
	ErrInvalidSignature = errors.New("invalid signature")
)

const ChallengeContext = "aime-auth-v1" // fips 204 domain seperation

func VerifyDeviceSignature(pubKey, nonce, sig []byte) error {
	if len(pubKey) != mldsa65.PublicKeySize {
		return ErrInvalidKey
	}

	pk := new(mldsa65.PublicKey)
	if err := pk.UnmarshalBinary(pubKey); err != nil {
		return ErrInvalidKey
	}

	if !mldsa65.Verify(pk, nonce, []byte(ChallengeContext), sig) {
		return ErrInvalidSignature
	}
	return nil
}

// --

const (
	argonTime    = 3
	argonMemory  = 64 * 1024
	argonThreads = 4
	argonKeyLen  = 32
	argonSaltLen = 16
)

func HashPassword(password string) (hash, salt []byte) {
	salt = make([]byte, argonSaltLen)
	_, _ = rand.Read(salt) // docs say it never return a error
	hash = argon2.IDKey([]byte(password), salt, argonTime, argonMemory, argonThreads, argonKeyLen)
	return hash, salt
}

func VerifyPassword(password string, hash, salt []byte) bool {
	if len(salt) != argonSaltLen || len(hash) != argonKeyLen {
		return false
	}

	computed := argon2.IDKey([]byte(password), salt, argonTime, argonMemory, argonThreads, argonKeyLen)
	return subtle.ConstantTimeCompare(hash, computed) == 1
}

// --

func NewToken() (token string, hash []byte) {
	token = rand.Text()
	return token, HashToken(token)
}

func HashToken(token string) []byte {
	sum := sha256.Sum256([]byte(token))
	return sum[:]
}
