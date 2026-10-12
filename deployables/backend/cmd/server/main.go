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
	"github.com/rapierxbox/aime/backend/internal/billing"
	"github.com/rapierxbox/aime/backend/internal/blobs"
	"github.com/rapierxbox/aime/backend/internal/config"
	"github.com/rapierxbox/aime/backend/internal/encode"
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

	// deferred cleanups dont run, same as log.Fatalf
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

	logger.Info("loading encoder", "backend", config.Backend, "model_dir", config.ModelDir)
	encoder, err := encode.Open(encode.Config{Backend: config.Backend, ModelDir: config.ModelDir, Pooling: config.Pooling, OnnxLib: config.OnnxLib})
	if err != nil {
		fatal("opening encoder failed", err)
	}
	defer encoder.Close()

	m := metrics.New()
	m.RegisterDBPool(func() metrics.DBPoolStats {
		st := store.PoolStat()
		return metrics.DBPoolStats{
			Total: int64(st.TotalConns()), Idle: int64(st.IdleConns()), Acquired: int64(st.AcquiredConns()), Constructing: int64(st.ConstructingConns()),
			AcquireCount: st.AcquireCount(), EmptyAcquireCount: st.EmptyAcquireCount(), AcquireWait: st.AcquireDuration(),
		}
	})
	go runJanitor(ctx, store, m, logger)

	ledger := billing.NewLedger(store, billing.DefaultPricing, config.BillingEnforce)
	if !config.BillingEnforce {
		logger.Warn("billing is not enforced, empty accounts can still run inference")
	}
	go runStorageBilling(ctx, ledger, m, logger)

	blobDir, err := blobs.Open(config.BackupDir)
	if err != nil {
		fatal("opening backup dir failed", err)
	}
	go runBackupSweep(ctx, store, blobDir, config.BackupTimeout, m, logger)

	api := httpapi.New(config, store, logger, m, encoder, ledger, blobDir)

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
	logger.Info("listening", "addr", config.HTTPAddr, "env", config.Env, "backend", encoder.Info().Backend)

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

// also refreshes the table count gauges, same cadence
func runJanitor(ctx context.Context, st *store.Store, m *metrics.Metrics, logger *slog.Logger) {
	const every = 5 * time.Minute
	ticker := time.NewTicker(every)
	defer ticker.Stop()
	for {
		purgeCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
		purged, err := st.PurgeExpired(purgeCtx)
		if err != nil && ctx.Err() == nil {
			logger.Error("janitor failed", "error", err)
		} else if purged.Total() > 0 {
			m.AddPurged("auth_challanges", purged.Challenges)
			m.AddPurged("enrollment_tokens", purged.EnrollmentTokens)
			m.AddPurged("sessions", purged.Sessions)
			logger.Info("janitor purged expired rows", "challanges", purged.Challenges, "enrollment_tokens", purged.EnrollmentTokens, "sessions", purged.Sessions)
		}
		if counts, err := st.TableCounts(purgeCtx); err != nil && ctx.Err() == nil {
			logger.Error("table counts failed", "error", err)
		} else if err == nil {
			m.SetTableCounts(metrics.TableCounts(counts))
		}
		cancel()
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

// storage is billed per account once its last bill is a day old; the hourly tick just checks who is due
func runStorageBilling(ctx context.Context, ledger *billing.Ledger, m *metrics.Metrics, logger *slog.Logger) {
	ticker := time.NewTicker(time.Hour)
	defer ticker.Stop()
	for {
		billCtx, cancel := context.WithTimeout(ctx, 5*time.Minute)
		rep, err := ledger.BillStorage(billCtx, time.Now(), logger)
		cancel()
		m.AddStorageBilled(rep.Accounts, rep.Microcredits)
		if err != nil && ctx.Err() == nil {
			logger.Error("storage billing failed", "error", err)
		} else if rep.Accounts > 0 {
			logger.Info("storage billing pass", "accounts", rep.Accounts, "microcredits", rep.Microcredits)
		}
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

// files without a row: crashed uploads, failed deletes. anything younger than one
// upload timeout may still be in flight and is skipped
func runBackupSweep(ctx context.Context, st *store.Store, dir *blobs.Dir, minAge time.Duration, m *metrics.Metrics, logger *slog.Logger) {
	ticker := time.NewTicker(time.Hour)
	defer ticker.Stop()
	for {
		sweepCtx, cancel := context.WithTimeout(ctx, 5*time.Minute)
		known, err := st.AllStoragePaths(sweepCtx)
		cancel()
		if err != nil {
			if ctx.Err() == nil {
				logger.Error("backup sweep: listing paths failed", "error", err)
			}
		} else {
			res, err := dir.Sweep(func(rel string) bool { _, ok := known[rel]; return ok }, minAge)
			if err != nil {
				logger.Error("backup sweep failed", "error", err)
			}
			if res.Files > 0 {
				m.AddSwept(res.Files, res.Bytes)
				logger.Info("backup sweep removed orphans", "files", res.Files, "bytes", res.Bytes)
			}
		}
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}
