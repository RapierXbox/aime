package httpapi

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/rapierxbox/aime/backend/internal/billing"
	"github.com/rapierxbox/aime/backend/internal/store"
)

// backups are opaque, client encrypted blobs. the body is the raw file; everything else travels in headers
const (
	backupChecksumHeader = "X-Backup-Checksum" // hex sha256 of the body, optional; upload fails if it does not match
	backupMetaHeader     = "X-Backup-Meta"     // json the client needs to decrypt (kdf params, nonce ...), opaque to us
	backupParentHeader   = "X-Backup-Parent"   // id of the backup this one is a delta of, optional
	maxBackupMeta        = 4 << 10
)

type backupRes struct {
	ID        int64           `json:"id"`
	Version   int             `json:"version"`
	Kind      string          `json:"kind"` // full | delta
	ParentID  *int64          `json:"parent_id,omitempty"`
	SizeBytes int64           `json:"size_bytes"`
	Checksum  string          `json:"checksum"`
	Meta      json.RawMessage `json:"meta"`
	CreatedAt time.Time       `json:"created_at"`
}

func toBackupRes(b store.Backup) backupRes {
	return backupRes{ID: b.ID, Version: b.Version, Kind: b.Kind, ParentID: b.ParentID, SizeBytes: b.SizeBytes, Checksum: b.Checksum, Meta: b.Meta, CreatedAt: b.CreatedAt}
}

type backupListRes struct {
	Backups          []backupRes `json:"backups"` // newest first
	StorageUsedBytes int64       `json:"storage_used_bytes"`
}

// GET /v1/backups
func (s *Server) handleListBackups(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}
	list, err := s.Store.ListBackups(r.Context(), accountID)
	if err != nil {
		s.Log.Error("failed to list backups", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	res := backupListRes{Backups: make([]backupRes, 0, len(list))}
	for _, b := range list {
		res.Backups = append(res.Backups, toBackupRes(b))
		res.StorageUsedBytes += b.SizeBytes
	}
	s.writeJSON(w, http.StatusOK, res)
}

// --

type backupUploadRes struct {
	backupRes
	StorageUsedBytes int64 `json:"storage_used_bytes"`
}

// POST /v1/backups
func (s *Server) handleUploadBackup(w http.ResponseWriter, r *http.Request) {
	result := "error"
	defer func() { s.Metrics.IncBackupOp("upload", result) }()

	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}
	maxBytes := s.Cfg.MaxBackupMB << 20

	// a declared length is the only thing that lets us refuse before the bytes flow;
	// go caps the body reader at Content-Length, so a client cannot exceed what it declared
	if r.ContentLength < 0 {
		result = "rejected"
		s.writeError(w, http.StatusLengthRequired, "Content-Length required")
		return
	}
	if r.ContentLength > maxBytes {
		result = "rejected"
		s.writeError(w, http.StatusRequestEntityTooLarge, fmt.Sprintf("backup too large, max %d MB", s.Cfg.MaxBackupMB))
		return
	}
	if r.ContentLength == 0 {
		result = "rejected"
		s.writeError(w, http.StatusBadRequest, "empty body")
		return
	}
	meta, err := parseBackupMeta(r.Header.Get(backupMetaHeader))
	if err != nil {
		result = "rejected"
		s.writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	want := strings.ToLower(r.Header.Get(backupChecksumHeader))
	if want != "" && !isHexSHA256(want) {
		result = "rejected"
		s.writeError(w, http.StatusBadRequest, backupChecksumHeader+" must be hex sha256")
		return
	}
	parentID, err := parseBackupParent(r.Header.Get(backupParentHeader))
	if err != nil {
		result = "rejected"
		s.writeError(w, http.StatusBadRequest, err.Error())
		return
	}

	// cheap early rejections; CreateBackup repeats them under the account lock
	used, err := s.Store.StorageUsed(r.Context(), accountID)
	if err != nil {
		s.Log.Error("failed to read storage usage", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	if s.Billing.StorageBillable(used + r.ContentLength) {
		if err := s.Billing.Precheck(r.Context(), accountID); err != nil {
			result = s.rejectStorage(w, err)
			return
		}
	}
	n, err := s.Store.CountBackups(r.Context(), accountID)
	if err != nil {
		s.Log.Error("failed to count backups", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	if n >= s.Cfg.BackupMaxVersions {
		result = "rejected"
		s.writeError(w, http.StatusConflict, "backup limit reached, delete one first")
		return
	}

	// one upload per account, a few server wide: bounds .part files on disk
	release, ok := s.acquireUpload(accountID)
	if !ok {
		result = "rejected"
		w.Header().Set("Retry-After", "30")
		s.writeError(w, http.StatusTooManyRequests, "an upload is already running")
		return
	}
	defer release()

	// the server wide 60s read/write timeouts are far too short for a multi GB body
	rc := http.NewResponseController(w)
	if err := rc.SetReadDeadline(time.Now().Add(s.Cfg.BackupTimeout)); err != nil {
		s.Log.Warn("cannot extend read deadline", "error", err)
	}
	if err := rc.SetWriteDeadline(time.Now().Add(s.Cfg.BackupTimeout)); err != nil {
		s.Log.Warn("cannot extend write deadline", "error", err)
	}

	rel, size, sum, err := s.Blobs.Write(accountID, http.MaxBytesReader(w, r.Body, maxBytes))
	if err != nil {
		var tooBig *http.MaxBytesError
		if errors.As(err, &tooBig) {
			result = "rejected"
			s.writeError(w, http.StatusRequestEntityTooLarge, fmt.Sprintf("backup too large, max %d MB", s.Cfg.MaxBackupMB))
			return
		}
		s.Log.Error("failed to store backup", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	discard := func() {
		if err := s.Blobs.Remove(rel); err != nil {
			s.Log.Error("failed to remove rejected backup file", "error", err, "path", rel)
		}
	}

	s.Metrics.AddBackupBytes("in", size)
	if size == 0 {
		discard()
		result = "rejected"
		s.writeError(w, http.StatusBadRequest, "empty body")
		return
	}
	if want != "" && want != sum {
		discard()
		result = "rejected"
		s.writeError(w, http.StatusBadRequest, "checksum mismatch")
		return
	}

	b, err := s.Store.CreateBackup(r.Context(), accountID, s.backupQuota(), s.Billing, store.NewBackup{
		ParentID:    parentID,
		StoragePath: rel,
		SizeBytes:   size,
		Checksum:    sum,
		Meta:        meta,
	})
	switch {
	case errors.Is(err, store.ErrBackupLimit):
		discard()
		result = "rejected"
		s.writeError(w, http.StatusConflict, "backup limit reached, delete one first")
		return
	case errors.Is(err, store.ErrStorageQuota):
		discard()
		result = s.rejectStorage(w, billing.ErrInsufficientCredits)
		return
	case errors.Is(err, store.ErrStorageFull):
		discard()
		result = "rejected"
		s.writeError(w, http.StatusInsufficientStorage, "server storage is full")
		return
	case errors.Is(err, store.ErrNotFound) && parentID != nil:
		discard()
		result = "rejected"
		s.writeError(w, http.StatusBadRequest, "unknown parent backup")
		return
	case errors.Is(err, store.ErrBackupStale):
		discard()
		result = "rejected"
		s.writeError(w, http.StatusConflict, "parent is not the latest backup, pull first")
		return
	case errors.Is(err, store.ErrNotFound):
		discard()
		s.writeUnauthorized(w)
		return
	case err != nil:
		discard()
		s.Log.Error("failed to record backup", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	result = "ok"
	s.writeJSON(w, http.StatusCreated, backupUploadRes{backupRes: toBackupRes(b), StorageUsedBytes: used + size})
}

func (s *Server) backupQuota() store.Quota {
	return store.Quota{
		MaxVersions:   s.Cfg.BackupMaxVersions,
		FreeBytes:     s.Billing.FreeStorageBytes(),
		Enforce:       s.Cfg.BillingEnforce,
		TotalCapBytes: s.Cfg.BackupTotalCapGB << 30,
	}
}

// acquireUpload: one in flight per account, BACKUP_CONCURRENCY server wide
func (s *Server) acquireUpload(accountID int64) (release func(), ok bool) {
	select {
	case s.uploadSem <- struct{}{}:
	default:
		return nil, false
	}
	s.uploadMu.Lock()
	if _, busy := s.uploading[accountID]; busy {
		s.uploadMu.Unlock()
		<-s.uploadSem
		return nil, false
	}
	s.uploading[accountID] = struct{}{}
	s.uploadMu.Unlock()
	return func() {
		s.uploadMu.Lock()
		delete(s.uploading, accountID)
		s.uploadMu.Unlock()
		<-s.uploadSem
	}, true
}

// rejectStorage writes the precheck failure and returns the metric result label
func (s *Server) rejectStorage(w http.ResponseWriter, err error) string {
	if errors.Is(err, billing.ErrInsufficientCredits) {
		s.Metrics.IncRejected("backup")
		s.writeInferError(w, err)
		return "rejected"
	}
	s.writeInferError(w, err)
	return "error"
}

// parseBackupMeta validates and compacts the header json so it is safe to echo back in a header
func parseBackupMeta(raw string) ([]byte, error) {
	if raw == "" {
		return []byte("{}"), nil
	}
	if len(raw) > maxBackupMeta {
		return nil, fmt.Errorf("%s too large, max %d bytes", backupMetaHeader, maxBackupMeta)
	}
	for i := 0; i < len(raw); i++ {
		if raw[i] < 0x20 || raw[i] > 0x7e {
			return nil, fmt.Errorf("%s must be printable ascii, base64 binary values", backupMetaHeader)
		}
	}
	var buf bytes.Buffer
	if err := json.Compact(&buf, []byte(raw)); err != nil || buf.Len() == 0 || buf.Bytes()[0] != '{' {
		return nil, fmt.Errorf("%s must be a json object", backupMetaHeader)
	}
	return buf.Bytes(), nil
}

func parseBackupParent(raw string) (*int64, error) {
	if raw == "" {
		return nil, nil
	}
	id, err := strconv.ParseInt(raw, 10, 64)
	if err != nil || id <= 0 {
		return nil, fmt.Errorf("%s must be a backup id", backupParentHeader)
	}
	return &id, nil
}

func isHexSHA256(s string) bool {
	if len(s) != 64 {
		return false
	}
	_, err := hex.DecodeString(s)
	return err == nil
}

// --

func (s *Server) backupIDFromPath(w http.ResponseWriter, r *http.Request) (accountID, id int64, ok bool) {
	accountID, ok = AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return 0, 0, false
	}
	id, err := strconv.ParseInt(r.PathValue("id"), 10, 64)
	if err != nil || id <= 0 {
		s.writeError(w, http.StatusBadRequest, "invalid backup id")
		return 0, 0, false
	}
	return accountID, id, true
}

// GET /v1/backups/{id}
func (s *Server) handleDownloadBackup(w http.ResponseWriter, r *http.Request) {
	result := "error"
	defer func() { s.Metrics.IncBackupOp("download", result) }()

	accountID, id, ok := s.backupIDFromPath(w, r)
	if !ok {
		result = "rejected"
		return
	}
	b, err := s.Store.GetBackup(r.Context(), accountID, id)
	if errors.Is(err, store.ErrNotFound) {
		result = "rejected"
		s.writeError(w, http.StatusNotFound, "unknown backup")
		return
	}
	if err != nil {
		s.Log.Error("failed to load backup", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	f, err := s.Blobs.Open(b.StoragePath)
	if err != nil {
		// row without file: our inconsistency, not the clients 404
		s.Log.Error("backup file missing", "error", err, "backup_id", b.ID, "path", b.StoragePath, "not_exist", errors.Is(err, fs.ErrNotExist))
		s.writeError(w, http.StatusInternalServerError, "backup unavailable")
		return
	}
	defer f.Close()

	rc := http.NewResponseController(w)
	if err := rc.SetWriteDeadline(time.Now().Add(s.Cfg.BackupTimeout)); err != nil {
		s.Log.Warn("cannot extend write deadline", "error", err)
	}
	if err := rc.SetReadDeadline(time.Now().Add(s.Cfg.BackupTimeout)); err != nil {
		s.Log.Warn("cannot extend read deadline", "error", err)
	}
	w.Header().Set("Content-Type", "application/octet-stream")
	w.Header().Set("Content-Disposition", fmt.Sprintf(`attachment; filename="backup-v%d.bin"`, b.Version))
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set(backupChecksumHeader, b.Checksum)
	w.Header().Set(backupMetaHeader, string(b.Meta)) // compacted json, single line
	if b.ParentID != nil {
		w.Header().Set(backupParentHeader, strconv.FormatInt(*b.ParentID, 10))
	}
	http.ServeContent(w, r, "", b.CreatedAt, f)
	result = "ok"
	s.Metrics.AddBackupBytes("out", b.SizeBytes) // range requests count the full size, close enough
}

// DELETE /v1/backups/{id}
func (s *Server) handleDeleteBackup(w http.ResponseWriter, r *http.Request) {
	result := "error"
	defer func() { s.Metrics.IncBackupOp("delete", result) }()

	accountID, id, ok := s.backupIDFromPath(w, r)
	if !ok {
		result = "rejected"
		return
	}
	b, err := s.Store.DeleteBackup(r.Context(), accountID, id, s.Billing)
	switch {
	case errors.Is(err, store.ErrNotFound):
		result = "rejected"
		s.writeError(w, http.StatusNotFound, "unknown backup")
		return
	case errors.Is(err, store.ErrBackupInUse):
		result = "rejected"
		s.writeError(w, http.StatusConflict, "backup is the parent of a delta, delete the delta first")
		return
	case err != nil:
		s.Log.Error("failed to delete backup", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}
	if err := s.Blobs.Remove(b.StoragePath); err != nil {
		// row is gone, the file is an orphan now; nothing the client can do about it
		s.Log.Error("failed to remove backup file", "error", err, "path", b.StoragePath)
	}
	result = "ok"
	w.WriteHeader(http.StatusNoContent)
}

type pruneRes struct {
	Deleted    int   `json:"deleted"`
	FreedBytes int64 `json:"freed_bytes"`
}

// DELETE /v1/backups?before_version=N drops every version below N. N must be a full
// backup so the chain that stays behind does not start with a delta
func (s *Server) handlePruneBackups(w http.ResponseWriter, r *http.Request) {
	result := "error"
	defer func() { s.Metrics.IncBackupOp("prune", result) }()

	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}
	before, err := strconv.Atoi(r.URL.Query().Get("before_version"))
	if err != nil || before <= 0 {
		result = "rejected"
		s.writeError(w, http.StatusBadRequest, "before_version must be a version number")
		return
	}

	pruned, err := s.Store.PruneBackups(r.Context(), accountID, before, s.Billing)
	switch {
	case errors.Is(err, store.ErrNotFound):
		result = "rejected"
		s.writeError(w, http.StatusNotFound, "unknown version")
		return
	case errors.Is(err, store.ErrBackupBoundary):
		result = "rejected"
		s.writeError(w, http.StatusConflict, "before_version is a delta; prune from a full backup")
		return
	case err != nil:
		s.Log.Error("failed to prune backups", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
		return
	}

	res := pruneRes{Deleted: len(pruned)}
	for _, b := range pruned {
		res.FreedBytes += b.SizeBytes
		if err := s.Blobs.Remove(b.StoragePath); err != nil {
			s.Log.Error("failed to remove pruned backup file", "error", err, "path", b.StoragePath) // sweep gets it
		}
	}
	result = "ok"
	s.writeJSON(w, http.StatusOK, res)
}
