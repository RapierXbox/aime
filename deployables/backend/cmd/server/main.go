package main

import (
	"context"
	"database/sql"
	"embed"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	_ "github.com/jackc/pgx/v5/stdlib"
	"github.com/pressly/goose/v3"
	"github.com/rapierxbox/aime/backend/internal/config"
	"github.com/rapierxbox/aime/backend/internal/httpapi"
	"github.com/rapierxbox/aime/backend/internal/metrics"
	"github.com/rapierxbox/aime/backend/internal/store"
)

//go:embed migrations/*.sql
var migrationsFS embed.FS

func migrate(databaseURL string) error {
	db, err := sql.Open("pgx", databaseURL)
	if err != nil {
		return err
	}
	defer db.Close()
	goose.SetBaseFS(migrationsFS)
	if err := goose.SetDialect("postgres"); err != nil {
		return err
	}
	return goose.Up(db, "migrations")
}

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))

	// fatal logs and exits; deferred cleanups dont run, same as log.Fatalf did
	fatal := func(msg string, err error) {
		logger.Error(msg, "error", err)
		os.Exit(1)
	}

	logger.Info("loading config")
	config, err := config.Load()
	if err != nil {
		fatal("loading config failed", err)
	}

	logger.Info("applying db migrations")
	if err := migrate(config.DatabaseURL); err != nil {
		fatal("running db migrations failed", err)
	}

	logger.Info("connecting to postgres")
	dbCtx, dbCancel := context.WithTimeout(ctx, 10*time.Second)
	defer dbCancel()
	store, err := store.Open(dbCtx, config.DatabaseURL)
	if err != nil {
		fatal("connecting to db pool failed", err)
	}
	defer store.Close()

	api := &httpapi.Server{
		Store:   store,
		Cfg:     config,
		Log:     logger,
		Metrics: metrics.New(),
	}

	srv := &http.Server{
		Addr:              config.HTTPAddr,
		Handler:           api.AccessLog(api.Recover(api.Routes())),
		ReadHeaderTimeout: 10 * time.Second, // slowloris
		WriteTimeout:      60 * time.Second,
		ReadTimeout:       60 * time.Second,
		IdleTimeout:       120 * time.Second,
	}

	errChan := make(chan error, 1)
	go func() {
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errChan <- err
		}
	}()
	logger.Info("listening", "addr", config.HTTPAddr, "env", config.Env)

	select {
	case err := <-errChan:
		fatal("server error", err)
	case <-ctx.Done():
		logger.Info("shutting down")
	}

	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer shutdownCancel()
	if err := srv.Shutdown(shutdownCtx); err != nil {
		fatal("server shutdown error", err)
	}
}
