package httpapi

import (
	"errors"
	"net/http"
	"strconv"
	"time"
	"unicode/utf8"

	"github.com/rapierxbox/aime/backend/internal/auth"
	"github.com/rapierxbox/aime/backend/internal/store"
)

type accountRes struct {
	AccountID           int64     `json:"account_id"`
	Email               string    `json:"email"`
	BalanceMicrocredits int64     `json:"balance_microcredits"`
	CreatedAt           time.Time `json:"created_at"`
}

// GET /v1/account
func (s *Server) handleGetAccount(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}

	acct, err := s.Store.AccountByID(r.Context(), accountID)
	if errors.Is(err, store.ErrNotFound) {
		// account deleted under a live session
		s.writeUnauthorized(w)
		return
	}
	if err != nil {
		s.Log.Error("failed to load account", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	s.writeJSON(w, http.StatusOK, accountRes{
		AccountID:           acct.ID,
		Email:               acct.Email,
		BalanceMicrocredits: acct.BalanceMicrocredits,
		CreatedAt:           acct.CreatedAt,
	})
}

// --

const (
	usageDaysDefault = 30
	usageDaysMax     = 365
)

type usageRow struct {
	Kind             string `json:"kind"` // embed | rerank | storage
	Model            string `json:"model"`
	Unit             string `json:"unit"` // tokens | mb_day
	Calls            int64  `json:"calls"`
	Quantity         int64  `json:"quantity"`
	CostMicrocredits int64  `json:"cost_microcredits"`
}

type usageSummaryRes struct {
	Since time.Time  `json:"since"`
	Rows  []usageRow `json:"rows"`
	Total struct {
		Calls            int64 `json:"calls"`
		CostMicrocredits int64 `json:"cost_microcredits"`
	} `json:"total"`
}

// GET /v1/account/usage?days=30
func (s *Server) handleGetUsage(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}

	days := usageDaysDefault
	if q := r.URL.Query().Get("days"); q != "" {
		n, err := strconv.Atoi(q)
		if err != nil || n < 1 || n > usageDaysMax {
			s.writeError(w, http.StatusBadRequest, "days must be 1..365")
			return
		}
		days = n
	}
	since := time.Now().Add(-time.Duration(days) * 24 * time.Hour)

	rows, err := s.Store.UsageSummary(r.Context(), accountID, since)
	if err != nil {
		s.Log.Error("failed to load usage", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	res := usageSummaryRes{Since: since, Rows: make([]usageRow, 0, len(rows))}
	for _, row := range rows {
		res.Rows = append(res.Rows, usageRow(row))
		res.Total.Calls += row.Calls
		res.Total.CostMicrocredits += row.CostMicrocredits
	}
	s.writeJSON(w, http.StatusOK, res)
}

// --

// POST /v1/auth/logout
func (s *Server) handleLogout(w http.ResponseWriter, r *http.Request) {
	tokenHash, ok := sessionFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}
	if err := s.Store.DeleteSession(r.Context(), tokenHash); err != nil {
		s.Log.Error("failed to delete session", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	s.Metrics.IncAuth("logout")
	w.WriteHeader(http.StatusNoContent)
}

// --

// verifyAccountPassword is the shared gate for destructive account actions
func (s *Server) verifyAccountPassword(w http.ResponseWriter, r *http.Request, accountID int64, password string) bool {
	acct, err := s.Store.AccountAuth(r.Context(), accountID)
	if errors.Is(err, store.ErrNotFound) {
		s.writeUnauthorized(w)
		return false
	}
	if err != nil {
		s.Log.Error("failed to load account credentials", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return false
	}
	var valid bool
	if !s.withArgon(w, r, func() { valid = auth.VerifyPassword(password, acct.PasswordHash, acct.PasswordSalt) }) {
		return false
	}
	if !valid {
		s.Metrics.IncAuth("login_fail")
		s.writeError(w, http.StatusUnauthorized, "invalid password")
		return false
	}
	return true
}

type changePasswordReq struct {
	CurrentPassword string `json:"current_password"`
	NewPassword     string `json:"new_password"`
}

// POST /v1/account/password: every other session is revoked, the calling one stays
func (s *Server) handleChangePassword(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}
	var req changePasswordReq
	if err := s.decodeJSON(w, r, &req); err != nil {
		return
	}
	if utf8.RuneCountInString(req.NewPassword) < minPasswordLen {
		s.writeError(w, http.StatusBadRequest, "password too short")
		return
	}
	if !s.verifyAccountPassword(w, r, accountID, req.CurrentPassword) {
		return
	}

	var hash, salt []byte
	if !s.withArgon(w, r, func() { hash, salt = auth.HashPassword(req.NewPassword) }) {
		return
	}
	if err := s.Store.UpdatePassword(r.Context(), accountID, hash, salt); err != nil {
		s.Log.Error("failed to update password", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	keep, _ := sessionFromContext(r.Context())
	if _, err := s.Store.DeleteSessions(r.Context(), accountID, keep); err != nil {
		// password is changed; a leftover session is the smaller problem, but say so
		s.Log.Error("failed to revoke other sessions after password change", "error", err, "account_id", accountID)
	}
	s.Metrics.IncAuth("password_change")
	w.WriteHeader(http.StatusNoContent)
}

type deleteAccountReq struct {
	Password string `json:"password"`
}

// DELETE /v1/account: everything goes, rows by cascade and blobs right after
func (s *Server) handleDeleteAccount(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}
	var req deleteAccountReq
	if err := s.decodeJSON(w, r, &req); err != nil {
		return
	}
	if !s.verifyAccountPassword(w, r, accountID, req.Password) {
		return
	}

	paths, err := s.Store.DeleteAccount(r.Context(), accountID)
	if errors.Is(err, store.ErrNotFound) {
		s.writeUnauthorized(w)
		return
	}
	if err != nil {
		s.Log.Error("failed to delete account", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	for _, p := range paths {
		if err := s.Blobs.Remove(p); err != nil {
			s.Log.Error("failed to remove backup file of deleted account", "error", err, "path", p) // sweep gets it
		}
	}
	s.Metrics.IncAuth("account_delete")
	w.WriteHeader(http.StatusNoContent)
}
