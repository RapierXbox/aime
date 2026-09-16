package httpapi

import (
	"context"
	"net/http"
	"time"
)

func (s *Server) handleHealth(w http.ResponseWriter, r *http.Request) {
	s.writeJSON(w, http.StatusOK, map[string]string{"status": "healthy"})
}

func (s *Server) handleReady(w http.ResponseWriter, r *http.Request) {
	pingCtx, cancel := context.WithTimeout(r.Context(), 2*time.Second)
	defer cancel()

	err := s.Store.Ping(pingCtx)
	if err != nil {
		s.Log.Error("readiness check failed", "error", err)

		s.writeJSON(w, http.StatusServiceUnavailable, map[string]string{"status": "degraded"})
		return
	}

	s.writeJSON(w, http.StatusOK, map[string]string{"status": "ready"})
}
