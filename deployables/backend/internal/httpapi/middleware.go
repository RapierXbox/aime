package httpapi

import (
	"context"
	"errors"
	"io"
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
	bytes  int64
}

func (sw *statusWriter) WriteHeader(status int) {
	sw.status = status
	sw.ResponseWriter.WriteHeader(status)
}

func (sw *statusWriter) Write(b []byte) (int, error) {
	n, err := sw.ResponseWriter.Write(b)
	sw.bytes += int64(n)
	return n, err
}

// lets http.NewResponseController reach the real writer (SetReadDeadline in the backup upload)
func (sw *statusWriter) Unwrap() http.ResponseWriter { return sw.ResponseWriter }

type countingBody struct {
	io.ReadCloser
	n int64
}

func (c *countingBody) Read(p []byte) (int, error) {
	n, err := c.ReadCloser.Read(p)
	c.n += int64(n)
	return n, err
}

// 404s have no pattern
func routeLabel(r *http.Request) string {
	if r.Pattern == "" {
		return "unmatched"
	}
	return r.Pattern
}

func (s *Server) AccessLog(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		sw := &statusWriter{ResponseWriter: w, status: 200}
		body := &countingBody{ReadCloser: r.Body}
		r.Body = body
		s.Metrics.HTTPInflight(1)
		defer func() {
			s.Metrics.HTTPInflight(-1)
			duration := time.Since(start)
			route := routeLabel(r)
			s.Metrics.ObserveHTTP(r.Method, route, sw.status, duration, body.n, sw.bytes)
			s.Log.Info("request completed", "method", r.Method, "path", route, "status", sw.status, "duration_ms", duration.Milliseconds(), "bytes_in", body.n, "bytes_out", sw.bytes)
		}()
		next.ServeHTTP(sw, r)
	})
}

// --
type ctxKey int

const (
	accountKey ctxKey = iota
	sessionKey        // token hash, for logout
	deviceKey         // device the session belongs to
)

func (s *Server) writeUnauthorized(w http.ResponseWriter) {
	w.Header().Set("WWW-Authenticate", `Bearer realm="aime"`)
	s.writeError(w, http.StatusUnauthorized, "unauthorized")
}

func (s *Server) RequireAuth(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		token, ok := strings.CutPrefix(r.Header.Get("Authorization"), "Bearer ")
		if !ok || token == "" {
			s.writeUnauthorized(w)
			return
		}

		tokenHash := auth.HashToken(token)
		sess, err := s.Store.SessionByTokenHash(r.Context(), tokenHash)
		if errors.Is(err, store.ErrNotFound) {
			s.writeUnauthorized(w)
			return
		}
		if err != nil {
			s.Log.Error("failed to get session by token hash", "error", err)
			s.writeError(w, http.StatusInternalServerError, "internal")
			return
		}

		ctx := context.WithValue(r.Context(), accountKey, sess.AccountID)
		ctx = context.WithValue(ctx, sessionKey, tokenHash)
		ctx = context.WithValue(ctx, deviceKey, sess.DeviceID)

		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

func AccountFromContext(ctx context.Context) (int64, bool) {
	accountID, ok := ctx.Value(accountKey).(int64)
	return accountID, ok
}

func sessionFromContext(ctx context.Context) ([]byte, bool) {
	hash, ok := ctx.Value(sessionKey).([]byte)
	return hash, ok
}

func deviceFromContext(ctx context.Context) (int64, bool) {
	id, ok := ctx.Value(deviceKey).(int64)
	return id, ok
}
