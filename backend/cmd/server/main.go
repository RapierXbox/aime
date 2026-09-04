package main

import (
	"context"
	"database/sql"
	"embed"
	"errors"
	"log"
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

	log.Print("Loading config...")
	config, err := config.Load()
	if err != nil {
		log.Fatalf("FATAL ERROR loading config: %s", err.Error())
	}

	log.Print("Applying DB migrations...")
	err = migrate(config.DatabaseURL)
	if err != nil {
		log.Fatalf("ERROR running DB migrations: %s", err.Error())
	}

	log.Print("Connecting to Postgres...")
	dbCtx, dbCancel := context.WithTimeout(ctx, 10*time.Second)
	defer dbCancel()
	store, err := store.Open(dbCtx, config.DatabaseURL)
	if err != nil {
		log.Fatalf("FATAL ERROR connecting to db pool: %s", err.Error())
	}
	defer store.Close()

	api := &httpapi.Server{
		Store: store,
		Cfg:   config,
		Log:   slog.New(slog.NewJSONHandler(os.Stdout, nil)),
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

	select {
	case err := <-errChan:
		log.Fatalf("server error: %s", err.Error())
	case <-ctx.Done():
		log.Print("shutting down")
	}

	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer shutdownCancel()
	if err := srv.Shutdown(shutdownCtx); err != nil {
		log.Fatalf("server shutdown error: %s", err.Error())
	}
}
