package httpapi

import (
	"net/http"
	"runtime/debug"
	"time"
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
