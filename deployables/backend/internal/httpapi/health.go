package httpapi

import (
	"context"
	"net/http"
	"sync"
	"time"
)

func (s *Server) handleHealth(w http.ResponseWriter, r *http.Request) {
	s.writeJSON(w, http.StatusOK, map[string]string{"status": "healthy"})
}

// readyz is public and unauthenticated; one db ping per second is plenty, more would just drain the pool
var ready struct {
	mu  sync.Mutex
	at  time.Time
	err error
}

const readyCacheFor = time.Second

func (s *Server) handleReady(w http.ResponseWriter, r *http.Request) {
	ready.mu.Lock()
	if time.Since(ready.at) > readyCacheFor {
		pingCtx, cancel := context.WithTimeout(r.Context(), 2*time.Second)
		ready.err = s.Store.Ping(pingCtx)
		cancel()
		ready.at = time.Now()
	}
	err := ready.err
	ready.mu.Unlock()

	if err != nil {
		s.Log.Error("readiness check failed", "error", err)
		s.writeJSON(w, http.StatusServiceUnavailable, map[string]string{"status": "degraded"})
		return
	}
	s.writeJSON(w, http.StatusOK, map[string]string{"status": "ready"})
}
