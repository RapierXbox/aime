package httpapi

import (
	"log/slog"
	"net/http"

	"github.com/rapierxbox/aime/backend/internal/config"
	"github.com/rapierxbox/aime/backend/internal/metrics"
	"github.com/rapierxbox/aime/backend/internal/store"
)

type Server struct {
	Store   *store.Store
	Cfg     *config.Config
	Log     *slog.Logger
	Metrics *metrics.Metrics
}

func (s *Server) Routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", s.handleHealth)
	mux.HandleFunc("GET /readyz", s.handleReady)
	mux.Handle("GET /metrics", s.Metrics.Handler())

	mux.HandleFunc("POST /v1/accounts", s.handleCreateAccount)
	mux.HandleFunc("POST /v1/auth/password", s.handleLoginPassword)
	mux.HandleFunc("POST /v1/devices", s.handleCreateDevice)
	mux.HandleFunc("POST /v1/auth/challange", s.handleChallange)
	mux.HandleFunc("POST /v1/auth/verify", s.handleVerify)

	protected := http.NewServeMux()
	protected.HandleFunc("POST /v1/enrollment-tokens", s.handleMintEnrollment)
	mux.Handle("/v1/", s.RequireAuth(protected))

	return mux
}
