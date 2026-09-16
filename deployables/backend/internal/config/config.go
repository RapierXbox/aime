package config

import (
	"errors"
	"fmt"
	"os"
	"strconv"
)

type Config struct {
	HTTPAddr    string
	DatabaseURL string
	Env         string // dev | int | prod
	Backend     string // onnx vs hailo
	MaxBackupMB int64
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
	max_backup_mb, err := strconv.Atoi(getenv("MAX_BACKUP_MB", "1024"))
	if err != nil {
		return nil, fmt.Errorf("MAX_BACKUP_MB invalid: %s - %s", getenv("MAX_BACKUP_MB", ""), err.Error())
	}
	return &Config{
		HTTPAddr:    httpaddr,
		DatabaseURL: database_url,
		Env:         env,
		Backend:     backend,
		MaxBackupMB: int64(max_backup_mb),
	}, nil
}

func getenv(key, fallback string) string {
	if value, ok := os.LookupEnv(key); ok {
		return value
	}
	return fallback
}
