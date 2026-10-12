package httpapi

import (
	"encoding/json"
	"errors"
	"net/http"
)

const maxJSONBody = 8 << 10 // 8KiB

func (s *Server) writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	err := json.NewEncoder(w).Encode(v)
	if err != nil {
		s.Log.Warn("failed to write json response", "error", err)
	}
}

func (s *Server) writeError(w http.ResponseWriter, status int, err string) {
	s.writeJSON(w, status, map[string]string{"error": err})
}

func (s *Server) decodeJSON(w http.ResponseWriter, r *http.Request, v any) error {
	return s.decodeJSONLimit(w, r, v, maxJSONBody)
}

func (s *Server) decodeJSONLimit(w http.ResponseWriter, r *http.Request, v any, limit int64) error {
	dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, limit))

	dec.DisallowUnknownFields() // catch unwanted fields
	err := dec.Decode(v)
	if err == nil && dec.More() {
		err = errors.New("trailing data after json value")
	}
	if err != nil {
		s.Log.Warn("failed to decode json request", "error", err)
		s.writeError(w, http.StatusBadRequest, "invalid json")
		return err
	}
	return nil
}
