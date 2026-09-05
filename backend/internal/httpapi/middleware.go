package httpapi

import (
	"context"
	"errors"
	"net/http"
	"runtime/debug"
	"strings"
	"time"

	"github.com/rapierxbox/aime/backend/internal/auth"
	"github.com/rapierxbox/aime/backend/internal/store"
)

func (s *Server) Recover(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer func() {
			if rec := recover(); rec != nil {
				if rec == http.ErrAbortHandler {
					panic(rec)
				}
				s.Log.Error("panic recovered", "panic", rec, "stack", string(debug.Stack()), "path", r.Pattern)
				s.writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "internal"})
			}
		}()
		next.ServeHTTP(w, r)
	})
}

// --

type statusWriter struct {
	http.ResponseWriter
	status int
}

func (sw *statusWriter) WriteHeader(status int) {
	sw.status = status
	sw.ResponseWriter.WriteHeader(status)
}

func (s *Server) AccessLog(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		sw := &statusWriter{ResponseWriter: w, status: 200}
		defer func() {
			duration := time.Since(start).Milliseconds()
			s.Log.Info("request completed", "method", r.Method, "path", r.Pattern, "status", sw.status, "duration_ms", duration)
		}()
		next.ServeHTTP(sw, r)
	})
}

// --
type ctxKey int

const accountKey ctxKey = 0

func (s *Server) RequireAuth(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := r.Header.Get("Authorization")

		token, ok := strings.CutPrefix(h, "Bearer ")
		if !ok {
			s.writeError(w, http.StatusBadRequest, "invalid token")
			return
		}

		sess, err := s.Store.SessionByTokenHash(r.Context(), auth.HashToken(token))
		if errors.Is(err, store.ErrNotFound) {
			s.writeError(w, http.StatusUnauthorized, "unauthorized")
			return
		}
		if err != nil {
			s.Log.Error("failed to get session by token hash", "error", err)
			s.writeError(w, http.StatusInternalServerError, "internal")
		}

		ctx := context.WithValue(r.Context(), accountKey, sess.AccountID)

		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

func AccountFromContext(ctx context.Context) (int64, bool) {
	accountID, ok := ctx.Value(accountKey).(int64)
	return accountID, ok
}
