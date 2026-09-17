package config

import (
	"errors"
	"fmt"
	"os"
	"strconv"
	"time"
)

type Config struct {
	HTTPAddr    string
	DatabaseURL string
	Env         string // dev | int | prod
	Backend     string // stub | onnx | hailo
	ModelDir    string // holds embed/ and rerank/
	Pooling     string // cls (bge) | mean (e5, minilm)
	OnnxLib     string // ONNXRUNTIME_LIB, path to the shared library
	MaxBackupMB int64  // per file

	BackupDir         string
	BackupMaxVersions int           // per account
	BackupTimeout     time.Duration // one upload or download
	BackupConcurrency int           // uploads in flight server wide
	BackupTotalCapGB  int64         // all accounts together, 0 = unlimited

	AuthConcurrency int // argon2 hashes at once, 64MiB each

	InferConcurrency int           // parallel model calls
	InferTimeout     time.Duration // per request, incl. queueing

	TrustProxy     bool // client ip from X-Forwarded-For (behind caddy)
	BillingEnforce bool // 402 on empty balance
}

func Load() (*Config, error) {
	httpaddr := getenv("HTTP_ADDR", ":8080")
	database_url, ok := os.LookupEnv("DATABASE_URL")
	if !ok {
		return nil, errors.New("DATABASE_URL not defined or empty!")
	}
	env := getenv("APP_ENV", "dev")
	switch env {
	case "dev", "int", "prod":
	default:
		return nil, fmt.Errorf("APP_ENV not valid... expected 'dev', 'int' or 'prod', got %s", env)
	}
	backend := getenv("BACKEND", "onnx")
	if env == "prod" && backend == "stub" {
		return nil, errors.New("BACKEND=stub serves fake vectors and is not allowed in prod")
	}
	model_dir := getenv("MODEL_DIR", "./models")
	pooling := getenv("EMBED_POOLING", "cls")
	if pooling != "cls" && pooling != "mean" {
		return nil, fmt.Errorf("EMBED_POOLING invalid: %s (need cls or mean)", pooling)
	}
	onnx_lib := getenv("ONNXRUNTIME_LIB", "")
	max_backup_mb, err := strconv.Atoi(getenv("MAX_BACKUP_MB", "1024"))
	if err != nil || max_backup_mb < 1 {
		return nil, fmt.Errorf("MAX_BACKUP_MB invalid: %s (need an integer >= 1)", getenv("MAX_BACKUP_MB", ""))
	}
	backup_dir := getenv("BACKUP_DIR", "./backups")
	backup_max_versions, err := strconv.Atoi(getenv("BACKUP_MAX_VERSIONS", "10"))
	if err != nil || backup_max_versions < 1 {
		return nil, fmt.Errorf("BACKUP_MAX_VERSIONS invalid: %s (need an integer >= 1)", getenv("BACKUP_MAX_VERSIONS", ""))
	}
	backup_timeout, err := time.ParseDuration(getenv("BACKUP_TIMEOUT", "30m"))
	if err != nil || backup_timeout <= 0 {
		return nil, fmt.Errorf("BACKUP_TIMEOUT invalid: %s (need a duration like 30m)", getenv("BACKUP_TIMEOUT", ""))
	}
	backup_concurrency, err := strconv.Atoi(getenv("BACKUP_CONCURRENCY", "4"))
	if err != nil || backup_concurrency < 1 {
		return nil, fmt.Errorf("BACKUP_CONCURRENCY invalid: %s (need an integer >= 1)", getenv("BACKUP_CONCURRENCY", ""))
	}
	backup_total_cap_gb, err := strconv.Atoi(getenv("BACKUP_TOTAL_CAP_GB", "0"))
	if err != nil || backup_total_cap_gb < 0 {
		return nil, fmt.Errorf("BACKUP_TOTAL_CAP_GB invalid: %s (need an integer >= 0)", getenv("BACKUP_TOTAL_CAP_GB", ""))
	}
	auth_concurrency, err := strconv.Atoi(getenv("AUTH_CONCURRENCY", "4"))
	if err != nil || auth_concurrency < 1 {
		return nil, fmt.Errorf("AUTH_CONCURRENCY invalid: %s (need an integer >= 1)", getenv("AUTH_CONCURRENCY", ""))
	}
	infer_concurrency, err := strconv.Atoi(getenv("INFER_CONCURRENCY", "2"))
	if err != nil || infer_concurrency < 1 {
		return nil, fmt.Errorf("INFER_CONCURRENCY invalid: %s (need an integer >= 1)", getenv("INFER_CONCURRENCY", ""))
	}
	infer_timeout, err := time.ParseDuration(getenv("INFER_TIMEOUT", "30s"))
	if err != nil || infer_timeout <= 0 {
		return nil, fmt.Errorf("INFER_TIMEOUT invalid: %s (need a duration like 30s)", getenv("INFER_TIMEOUT", ""))
	}
	trust_proxy, err := strconv.ParseBool(getenv("TRUST_PROXY", "false"))
	if err != nil {
		return nil, fmt.Errorf("TRUST_PROXY invalid: %s (need true or false)", getenv("TRUST_PROXY", ""))
	}
	billing_enforce, err := strconv.ParseBool(getenv("BILLING_ENFORCE", "true"))
	if err != nil {
		return nil, fmt.Errorf("BILLING_ENFORCE invalid: %s (need true or false)", getenv("BILLING_ENFORCE", ""))
	}
	return &Config{
		HTTPAddr:          httpaddr,
		DatabaseURL:       database_url,
		Env:               env,
		Backend:           backend,
		ModelDir:          model_dir,
		Pooling:           pooling,
		OnnxLib:           onnx_lib,
		MaxBackupMB:       int64(max_backup_mb),
		BackupDir:         backup_dir,
		BackupMaxVersions: backup_max_versions,
		BackupTimeout:     backup_timeout,
		BackupConcurrency: backup_concurrency,
		BackupTotalCapGB:  int64(backup_total_cap_gb),
		AuthConcurrency:   auth_concurrency,
		InferConcurrency:  infer_concurrency,
		InferTimeout:      infer_timeout,
		TrustProxy:        trust_proxy,
		BillingEnforce:    billing_enforce,
	}, nil
}

func getenv(key, fallback string) string {
	if value, ok := os.LookupEnv(key); ok {
		return value
	}
	return fallback
}
