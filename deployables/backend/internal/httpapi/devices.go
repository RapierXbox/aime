package httpapi

import (
	"errors"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/rapierxbox/aime/backend/internal/store"
)

type deviceRes struct {
	ID        int64      `json:"id"`
	Name      string     `json:"name"`
	PqAlg     string     `json:"pq_alg"`
	CreatedAt time.Time  `json:"created_at"`
	LastSeen  *time.Time `json:"last_seen,omitempty"`
	Current   bool       `json:"current"` // the device this request came from
}

// GET /v1/devices
func (s *Server) handleListDevices(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}
	current, _ := deviceFromContext(r.Context())

	devices, err := s.Store.ListDevices(r.Context(), accountID)
	if err != nil {
		s.Log.Error("failed to list devices", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	out := make([]deviceRes, 0, len(devices))
	for _, d := range devices {
		out = append(out, deviceRes{ID: d.ID, Name: d.Name, PqAlg: d.PqAlg, CreatedAt: d.CreatedAt, LastSeen: d.LastSeen, Current: d.ID == current})
	}
	s.writeJSON(w, http.StatusOK, map[string]any{"devices": out})
}

func (s *Server) deviceIDFromPath(w http.ResponseWriter, r *http.Request) (accountID, id int64, ok bool) {
	accountID, ok = AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return 0, 0, false
	}
	id, err := strconv.ParseInt(r.PathValue("id"), 10, 64)
	if err != nil || id <= 0 {
		s.writeError(w, http.StatusBadRequest, "invalid device id")
		return 0, 0, false
	}
	return accountID, id, true
}

type renameDeviceReq struct {
	Name string `json:"name"`
}

// PATCH /v1/devices/{id}
func (s *Server) handleRenameDevice(w http.ResponseWriter, r *http.Request) {
	accountID, id, ok := s.deviceIDFromPath(w, r)
	if !ok {
		return
	}
	var req renameDeviceReq
	if err := s.decodeJSON(w, r, &req); err != nil {
		return
	}
	if !validDeviceName(req.Name) {
		s.writeError(w, http.StatusBadRequest, "name must be 2..64 characters")
		return
	}

	err := s.Store.RenameDevice(r.Context(), accountID, id, strings.TrimSpace(req.Name))
	if errors.Is(err, store.ErrNotFound) {
		s.writeError(w, http.StatusNotFound, "unknown device")
		return
	}
	if err != nil {
		s.Log.Error("failed to rename device", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// DELETE /v1/devices/{id}: also ends every session of that device, including this one if it is the caller
func (s *Server) handleDeleteDevice(w http.ResponseWriter, r *http.Request) {
	accountID, id, ok := s.deviceIDFromPath(w, r)
	if !ok {
		return
	}
	err := s.Store.DeleteDevice(r.Context(), accountID, id)
	if errors.Is(err, store.ErrNotFound) {
		s.writeError(w, http.StatusNotFound, "unknown device")
		return
	}
	if err != nil {
		s.Log.Error("failed to delete device", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	s.Metrics.IncAuth("device_delete")
	w.WriteHeader(http.StatusNoContent)
}

// POST /v1/auth/logout-all revokes every session of the account, this one included
func (s *Server) handleLogoutAll(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}
	n, err := s.Store.DeleteSessions(r.Context(), accountID, nil)
	if err != nil {
		s.Log.Error("failed to revoke sessions", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	s.Metrics.IncAuth("logout_all")
	s.writeJSON(w, http.StatusOK, map[string]int64{"revoked": n})
}
