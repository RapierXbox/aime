package httpapi

import (
	"crypto/rand"
	"errors"
	"net/http"
	"net/mail"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/cloudflare/circl/sign/mldsa/mldsa65"
	"github.com/rapierxbox/aime/backend/internal/auth"
	"github.com/rapierxbox/aime/backend/internal/store"
)

const (
	challangeTTL   = 2 * time.Minute
	enrollmentTTL  = 10 * time.Minute
	sessionTTL     = 20 * 24 * time.Hour
	minPasswordLen = 12
)

var dummyHash, dummySalt = auth.HashPassword("timing-defense-placeholder")

const argonWait = 2 * time.Second

// withArgon bounds concurrent password hashing: 64MiB each, an unbounded burst would oom the box.
// returns false after writing a 503 when no slot frees up in time
func (s *Server) withArgon(w http.ResponseWriter, r *http.Request, fn func()) bool {
	select {
	case s.argonSem <- struct{}{}:
		defer func() { <-s.argonSem }()
		fn()
		return true
	case <-time.After(argonWait):
		w.Header().Set("Retry-After", "5")
		s.writeError(w, http.StatusServiceUnavailable, "busy, retry shortly")
		return false
	case <-r.Context().Done():
		return false
	}
}

func validDeviceName(name string) bool {
	n := utf8.RuneCountInString(strings.TrimSpace(name))
	return n >= 2 && n <= 64
}

// --

func NormalizeEmail(email string) (string, error) {
	emailAddr, err := mail.ParseAddress(email)
	if err != nil {
		return "", err
	}
	return strings.ToLower(emailAddr.Address), nil
}

// --

type createAccountReq struct {
	Email    string `json:"email"`
	Password string `json:"password"`
}
type createAccountRes struct {
	AccountID int64 `json:"account_id"`
}

func (s *Server) handleCreateAccount(w http.ResponseWriter, r *http.Request) {
	var req createAccountReq
	if err := s.decodeJSON(w, r, &req); err != nil {
		return
	}

	email, err := NormalizeEmail(req.Email)
	if err != nil {
		s.writeError(w, http.StatusBadRequest, "invalid email")
		return
	}

	if utf8.RuneCountInString(req.Password) < minPasswordLen {
		s.writeError(w, http.StatusBadRequest, "password too short")
		return
	}

	var hash, salt []byte
	if !s.withArgon(w, r, func() { hash, salt = auth.HashPassword(req.Password) }) {
		return
	}
	accountID, err := s.Store.CreateAccount(r.Context(), email, hash, salt)
	if errors.Is(err, store.ErrEmailTaken) {
		s.writeError(w, http.StatusConflict, "email already registred")
		return
	}
	if err != nil {
		s.Log.Error("failed to create account", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	s.Metrics.IncAuth("signup")
	s.writeJSON(w, http.StatusCreated, createAccountRes{AccountID: accountID})
}

// --

type loginPasswordReq struct {
	Email    string `json:"email"`
	Password string `json:"password"`
}

type loginPasswordRes struct {
	EnrollmentToken string    `json:"enrollment_token"`
	ExpiresAt       time.Time `json:"expires_at"`
}

func (s *Server) handleLoginPassword(w http.ResponseWriter, r *http.Request) {
	var req loginPasswordReq
	if err := s.decodeJSON(w, r, &req); err != nil {
		return
	}

	email, err := NormalizeEmail(req.Email)
	if err != nil {
		s.writeError(w, http.StatusBadRequest, "invalid email")
		return
	}

	acct, err := s.Store.AccountByEmail(r.Context(), email)
	if err != nil && !errors.Is(err, store.ErrNotFound) {
		// a db outage must not look like a wrong password
		s.Log.Error("failed to load account by email", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	hash, salt := acct.PasswordHash, acct.PasswordSalt
	if errors.Is(err, store.ErrNotFound) {
		hash, salt = dummyHash, dummySalt // same cost as a real check
	}
	var valid bool
	if !s.withArgon(w, r, func() { valid = auth.VerifyPassword(req.Password, hash, salt) }) {
		return
	}
	if !valid || errors.Is(err, store.ErrNotFound) {
		s.Metrics.IncAuth("login_fail")
		s.writeError(w, http.StatusUnauthorized, "invalid password")
		return
	}
	s.Metrics.IncAuth("login_ok")

	token, hash := auth.NewToken()
	err = s.Store.CreateEnrollmentToken(r.Context(), acct.ID, hash, "password", enrollmentTTL)
	if err != nil {
		s.Log.Error("failed to create enrollment token", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	s.writeJSON(w, http.StatusOK, loginPasswordRes{EnrollmentToken: token, ExpiresAt: time.Now().Add(enrollmentTTL)})
}

// --

type createDeviceReq struct {
	EnrollmentToken string `json:"enrollment_token"`
	Name            string `json:"name"`
	PublicKey       []byte `json:"public_key"`
}

type createDeviceRes struct {
	DeviceID int64 `json:"device_id"`
}

func (s *Server) handleCreateDevice(w http.ResponseWriter, r *http.Request) {
	var req createDeviceReq
	if err := s.decodeJSON(w, r, &req); err != nil {
		return
	}

	if !validDeviceName(req.Name) {
		s.writeError(w, http.StatusBadRequest, "name must be 2..64 characters")
		return
	}

	if len(req.PublicKey) != mldsa65.PublicKeySize {
		s.writeError(w, http.StatusBadRequest, "invalid public key")
		return
	}

	accountID, err := s.Store.ConsumeEnrollmentToken(r.Context(), auth.HashToken(req.EnrollmentToken))
	if errors.Is(err, store.ErrEnrollmentInvalid) {
		s.writeError(w, http.StatusUnauthorized, "invalid enrollment token")
		return
	}
	if err != nil {
		s.Log.Error("failed to consume enrollment token", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	deviceID, err := s.Store.CreateDevice(r.Context(), accountID, req.Name, "ml-dsa-65", req.PublicKey)
	if err != nil {
		s.Log.Error("failed to create device", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	s.Metrics.IncAuth("enroll")
	s.writeJSON(w, http.StatusCreated, createDeviceRes{DeviceID: deviceID})

}

// --

type challangeReq struct {
	DeviceID int64 `json:"device_id"`
}

type challangeRes struct {
	ChallangeID int64  `json:"challange_id"`
	Nonce       []byte `json:"nonce"`
}

func (s *Server) handleChallange(w http.ResponseWriter, r *http.Request) {
	var req challangeReq
	if err := s.decodeJSON(w, r, &req); err != nil {
		return
	}

	nonce := make([]byte, 32)
	rand.Read(nonce)

	challangeID, err := s.Store.CreateChallenge(r.Context(), req.DeviceID, nonce, challangeTTL)
	if errors.Is(err, store.ErrNotFound) {
		s.writeError(w, http.StatusNotFound, "unknown device")
		return
	}
	if err != nil {
		s.Log.Error("failed to create challange", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	s.writeJSON(w, http.StatusCreated, challangeRes{ChallangeID: challangeID, Nonce: nonce})
}

// --

type verifyReq struct {
	ChallangeID int64  `json:"challange_id"`
	Nonce       []byte `json:"nonce"` // the one from the challange; ids alone are guessable
	Signature   []byte `json:"signature"`
}

type verifyRes struct {
	Token     string    `json:"token"`
	ExpiresAt time.Time `json:"expires_at"`
}

func (s *Server) handleVerify(w http.ResponseWriter, r *http.Request) {
	var req verifyReq
	if err := s.decodeJSON(w, r, &req); err != nil {
		return
	}

	if len(req.Nonce) != 32 {
		s.writeError(w, http.StatusBadRequest, "invalid nonce")
		return
	}
	ch, err := s.Store.ConsumeChallenge(r.Context(), req.ChallangeID, req.Nonce)
	if errors.Is(err, store.ErrChallengeInvalid) {
		s.Metrics.IncAuth("verify_fail")
		s.writeError(w, http.StatusBadRequest, "invalid challange")
		return
	}
	if err != nil {
		s.Log.Error("failed to consume challange", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	dev, err := s.Store.GetDevice(r.Context(), ch.DeviceID)
	if errors.Is(err, store.ErrNotFound) {
		s.writeError(w, http.StatusBadRequest, "invalid device id")
		return
	}
	if err != nil {
		s.Log.Error("failed to get device", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	err = auth.VerifyDeviceSignature(dev.PublicKey, ch.Nonce, req.Signature)
	if err != nil {
		s.Metrics.IncAuth("verify_fail")
		s.writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	s.Metrics.IncAuth("verify_ok")

	token, hash := auth.NewToken()
	err = s.Store.CreateSession(r.Context(), dev.AccountID, dev.ID, hash, sessionTTL)
	if err != nil {
		s.Log.Error("failed to create session", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	err = s.Store.TouchDeviceLastSeen(r.Context(), dev.ID)
	if err != nil {
		s.Log.Error("failed to touch device last seen", "error", err)
	}

	s.writeJSON(w, http.StatusOK, verifyRes{Token: token, ExpiresAt: time.Now().Add(sessionTTL)})
}

// --

type mintEnrollmentReq struct {
	Password string `json:"password"` // adding a device is the one thing a stolen session must not be able to do
}

func (s *Server) handleMintEnrollment(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}
	var req mintEnrollmentReq
	if err := s.decodeJSON(w, r, &req); err != nil {
		return
	}
	if !s.verifyAccountPassword(w, r, accountID, req.Password) {
		return
	}

	token, hash := auth.NewToken()
	if err := s.Store.CreateEnrollmentToken(r.Context(), accountID, hash, "qr", enrollmentTTL); err != nil {
		s.Log.Error("failed to create enrollment token", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	s.writeJSON(w, http.StatusCreated, loginPasswordRes{EnrollmentToken: token, ExpiresAt: time.Now().Add(enrollmentTTL)})
}
