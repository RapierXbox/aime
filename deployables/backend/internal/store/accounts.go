package store

import (
	"context"
	"errors"
	"time"

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

type AccountInfo struct {
	ID                  int64
	Email               string
	BalanceMicrocredits int64
	CreatedAt           time.Time
}

func (s *Store) AccountByID(ctx context.Context, id int64) (AccountInfo, error) {
	var a AccountInfo
	err := s.pool.QueryRow(ctx,
		`SELECT id, email, balance_microcredits, created_at
		 FROM accounts
		 WHERE id = $1`,
		id,
	).Scan(&a.ID, &a.Email, &a.BalanceMicrocredits, &a.CreatedAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return AccountInfo{}, ErrNotFound
	}
	return a, err
}

// accountAuth is the credential view for password checks on an authenticated account
func (s *Store) AccountAuth(ctx context.Context, id int64) (Account, error) {
	var a Account
	err := s.pool.QueryRow(ctx,
		`SELECT id, email, password_hash, password_salt FROM accounts WHERE id = $1`,
		id,
	).Scan(&a.ID, &a.Email, &a.PasswordHash, &a.PasswordSalt)
	if errors.Is(err, pgx.ErrNoRows) {
		return Account{}, ErrNotFound
	}
	return a, err
}

func (s *Store) UpdatePassword(ctx context.Context, id int64, hash, salt []byte) error {
	tag, err := s.pool.Exec(ctx,
		`UPDATE accounts SET password_hash = $2, password_salt = $3 WHERE id = $1`,
		id, hash, salt,
	)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// deleteAccount cascades devices, sessions, tokens, usage and backup rows and returns
// the blob paths so the caller can remove the files; the sweep catches whatever fails
func (s *Store) DeleteAccount(ctx context.Context, id int64) ([]string, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return nil, err
	}
	defer tx.Rollback(ctx) //nolint:errcheck // no-op after commit

	rows, err := tx.Query(ctx, `SELECT storage_path FROM backups WHERE account_id = $1`, id)
	if err != nil {
		return nil, err
	}
	paths := []string{}
	for rows.Next() {
		var p string
		if err := rows.Scan(&p); err != nil {
			rows.Close()
			return nil, err
		}
		paths = append(paths, p)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return nil, err
	}

	tag, err := tx.Exec(ctx, `DELETE FROM accounts WHERE id = $1`, id)
	if err != nil {
		return nil, err
	}
	if tag.RowsAffected() == 0 {
		return nil, ErrNotFound
	}
	return paths, tx.Commit(ctx)
}
