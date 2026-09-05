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

func (s *Server) writeError(w http.ResponseWriter, status int, error string) {
	s.writeJSON(w, status, map[string]string{"error": "error"})
}

func (s *Server) decodeJSON(w http.ResponseWriter, r *http.Request, v any) error {
	dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, 8<<10)) // 8KiB limit

	dec.DisallowUnknownFields() // catch unwanted fields
	err := dec.Decode(v)
	if err != nil {
		s.Log.Warn("failed to decode json request", "error", err)
		s.writeError(w, http.StatusBadRequest, "invalid json")
		return err
	}
	return nil
}
