package httpapi

import (
	"log/slog"
	"net/http"
	"sync"
	"time"

	"golang.org/x/time/rate"

	"github.com/rapierxbox/aime/backend/internal/billing"
	"github.com/rapierxbox/aime/backend/internal/blobs"
	"github.com/rapierxbox/aime/backend/internal/config"
	"github.com/rapierxbox/aime/backend/internal/encode"
	"github.com/rapierxbox/aime/backend/internal/metrics"
	"github.com/rapierxbox/aime/backend/internal/store"
)

type Server struct {
	Store   *store.Store
	Cfg     *config.Config
	Log     *slog.Logger
	Metrics *metrics.Metrics
	Encoder encode.Encoder
	Billing billing.Biller
	Blobs   *blobs.Dir

	inferSem  chan struct{} // see infer.go
	argonSem  chan struct{} // see auth.go withArgon
	uploadSem chan struct{} // see backup.go acquireUpload
	uploadMu  sync.Mutex
	uploading map[int64]struct{}

	// per client ip; clients behind one nat/wg hub share a bucket
	limitAuth   *ipLimiter // login + signup (argon2)
	limitPublic *ipLimiter // other unauthenticated routes
}

func New(cfg *config.Config, st *store.Store, log *slog.Logger, m *metrics.Metrics, enc encode.Encoder, bill billing.Biller, bl *blobs.Dir) *Server {
	s := &Server{
		Store:       st,
		Cfg:         cfg,
		Log:         log,
		Metrics:     m,
		Encoder:     enc,
		Billing:     bill,
		Blobs:       bl,
		inferSem:    make(chan struct{}, cfg.InferConcurrency),
		argonSem:    make(chan struct{}, cfg.AuthConcurrency),
		uploadSem:   make(chan struct{}, cfg.BackupConcurrency),
		uploading:   make(map[int64]struct{}),
		limitAuth:   newIPLimiter(rate.Every(3*time.Second), 10), // 20/min sustained, burst 10
		limitPublic: newIPLimiter(rate.Limit(5), 30),             // 5/s sustained, burst 30
	}
	m.RegisterRateLimiter("auth", s.limitAuth.size)
	m.RegisterRateLimiter("public", s.limitPublic.size)
	return s
}

func (s *Server) Routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", s.handleHealth)
	mux.HandleFunc("GET /readyz", s.handleReady)
	mux.Handle("GET /metrics", s.Metrics.Handler()) // not behind caddy

	// wrapped per route so r.Pattern is the real route in logs/metrics
	strict := func(h http.HandlerFunc) http.Handler { return s.RateLimit(s.limitAuth, h) }
	public := func(h http.HandlerFunc) http.Handler { return s.RateLimit(s.limitPublic, h) }
	auth := func(h http.HandlerFunc) http.Handler { return s.RequireAuth(h) }

	// unauthenticated
	mux.Handle("POST /v1/accounts", strict(s.handleCreateAccount))
	mux.Handle("POST /v1/auth/password", strict(s.handleLoginPassword))
	mux.Handle("POST /v1/devices", public(s.handleCreateDevice))
	mux.Handle("POST /v1/auth/challange", public(s.handleChallange))
	mux.Handle("POST /v1/auth/verify", public(s.handleVerify))

	// session required
	mux.Handle("POST /v1/auth/logout", auth(s.handleLogout))
	mux.Handle("POST /v1/auth/logout-all", auth(s.handleLogoutAll))
	mux.Handle("POST /v1/enrollment-tokens", strict(auth(s.handleMintEnrollment).ServeHTTP)) // password re-check, argon2
	mux.Handle("GET /v1/devices", auth(s.handleListDevices))
	mux.Handle("PATCH /v1/devices/{id}", auth(s.handleRenameDevice))
	mux.Handle("DELETE /v1/devices/{id}", auth(s.handleDeleteDevice))
	mux.Handle("GET /v1/account", auth(s.handleGetAccount))
	mux.Handle("GET /v1/account/usage", auth(s.handleGetUsage))
	mux.Handle("POST /v1/account/password", strict(auth(s.handleChangePassword).ServeHTTP)) // argon2 twice
	mux.Handle("DELETE /v1/account", strict(auth(s.handleDeleteAccount).ServeHTTP))
	mux.Handle("POST /v1/embed", auth(s.handleEmbed))
	mux.Handle("POST /v1/rerank", auth(s.handleRerank))
	mux.Handle("GET /v1/backups", auth(s.handleListBackups))
	mux.Handle("POST /v1/backups", auth(s.handleUploadBackup))
	mux.Handle("GET /v1/backups/{id}", auth(s.handleDownloadBackup))
	mux.Handle("DELETE /v1/backups/{id}", auth(s.handleDeleteBackup))
	mux.Handle("DELETE /v1/backups", auth(s.handlePruneBackups))

	return mux
}
