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

	hash, salt := auth.HashPassword(req.Password)
	accountID, err := s.Store.CreateAccount(r.Context(), email, hash, salt)
	if errors.Is(err, store.ErrEmailTaken) {
		s.writeError(w, http.StatusConflict, "email already registred")
		return
	}
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

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
	if errors.Is(err, store.ErrNotFound) {
		auth.VerifyPassword(req.Password, dummyHash, dummySalt)
		s.writeError(w, http.StatusUnauthorized, "invalid password")
		return
	}

	if !auth.VerifyPassword(req.Password, acct.PasswordHash, acct.PasswordSalt) {
		s.writeError(w, http.StatusUnauthorized, "invalid password")
		return
	}

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

	if utf8.RuneCountInString(req.Name) <= 2 {
		s.writeError(w, http.StatusBadRequest, "name must be longer then 1")
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

	ch, err := s.Store.ConsumeChallenge(r.Context(), req.ChallangeID)
	if errors.Is(err, store.ErrChallengeInvalid) {
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
		s.writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}

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

func (s *Server) handleMintEnrollment(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeError(w, http.StatusUnauthorized, "unauthorized")
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
