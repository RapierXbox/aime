package main

import (
	"database/sql"
	"embed"
	"fmt"
	"log"

	_ "github.com/jackc/pgx/v5/stdlib"
	"github.com/pressly/goose/v3"
	"github.com/rapierxbox/aime/backend/internal/config"
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
	fmt.Println("hello world!!!!")

	log.Print("Loading config...")
	config, err := config.Load()
	if err != nil {
		log.Fatalf("FATAL ERROR loading config: %s", err.Error())
	}

	log.Print("Applying DB migrations...")
	err = migrate(config.DatabaseURL)
	if err != nil {
		log.Printf("ERROR running DB migrations: %s", err.Error())
	}

	for {

	}
}
