package httpapi

import (
	"encoding/json"
	"net/http"
)

func (s *Server) writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	err := json.NewEncoder(w).Encode(v)
	if err != nil {
		s.Log.Warn("failed to write json response", "error", err)
	}
}
