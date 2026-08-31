package httpapi

import (
	"context"
	"encoding/json"
	"net/http"
	"time"
)

func (s *Server) handleHealth(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(map[string]string{"status": "healthy"})
}

func (s *Server) handleReady(w http.ResponseWriter, r *http.Request) {
	pingCtx, cancel := context.WithTimeout(r.Context(), 2*time.Second)
	defer cancel()

	err := s.Store.Ping(pingCtx)
	if err != nil {
		s.Log.Error("readiness check failed", "error", err)

		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusServiceUnavailable)
		json.NewEncoder(w).Encode(map[string]string{"status": "degraded"})
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(map[string]string{"status": "ready"})
}
