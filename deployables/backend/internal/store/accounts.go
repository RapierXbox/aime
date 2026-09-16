package store

import (
	"context"
	"errors"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

var (
	ErrEmailTaken = errors.New("email already taken")
)

type Account struct {
	ID           int64
	Email        string
	PasswordHash []byte
	PasswordSalt []byte
}

func (s *Store) CreateAccount(ctx context.Context, email string, hash, salt []byte) (int64, error) {
	var id int64
	err := s.pool.QueryRow(ctx,
		`INSERT INTO accounts (email, password_hash, password_salt)
		 VALUES ($1, $2, $3)
		 RETURNING id`,
		email, hash, salt,
	).Scan(&id)

	var pgxErr *pgconn.PgError
	if errors.As(err, &pgxErr) && pgxErr.Code == "23505" {
		return 0, ErrEmailTaken
	}
	return id, err
}

func (s *Store) AccountByEmail(ctx context.Context, email string) (Account, error) {
	var a Account
	err := s.pool.QueryRow(ctx,
		`SELECT id, email, password_hash, password_salt
		 FROM accounts
		 WHERE email = $1`,
		email,
	).Scan(&a.ID, &a.Email, &a.PasswordHash, &a.PasswordSalt)
	if errors.Is(err, pgx.ErrNoRows) {
		return Account{}, ErrNotFound
	}
	return a, err
}
