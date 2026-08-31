package httpapi

import (
	"log/slog"
	"net/http"

	"github.com/prometheus/client_golang/prometheus/promhttp"
	"github.com/rapierxbox/aime/backend/internal/config"
	"github.com/rapierxbox/aime/backend/internal/store"
)

type Server struct {
	Store *store.Store
	Cfg   *config.Config
	Log   *slog.Logger
}

func (s *Server) Routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", s.handleHealth)
	mux.HandleFunc("GET /readyz", s.handleReady)
	mux.Handle("GET /metrics", promhttp.Handler())
	return mux
}
