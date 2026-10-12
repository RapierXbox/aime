package store

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
)

var (
	ErrEnrollmentInvalid = errors.New("enrollment invalid")
)

func (s *Store) CreateEnrollmentToken(ctx context.Context, accountID int64, tokenHash []byte, source string, ttl time.Duration) error {
	_, err := s.pool.Exec(ctx,
		`INSERT INTO enrollment_tokens (account_id, token_hash, source, expires_at)
		 VALUES ($1, $2, $3, now() + make_interval(secs => $4))`,
		accountID, tokenHash, source, ttl.Seconds(),
	)
	return err
}

func (s *Store) ConsumeEnrollmentToken(ctx context.Context, tokenHash []byte) (int64, error) {
	var accountID int64
	err := s.pool.QueryRow(ctx,
		`UPDATE enrollment_tokens
		 SET used = true
		 WHERE token_hash = $1 AND NOT used AND expires_at > now()
		 RETURNING account_id`,
		tokenHash,
	).Scan(&accountID)
	if errors.Is(err, pgx.ErrNoRows) {
		return 0, ErrEnrollmentInvalid
	}
	if err != nil {
		return 0, err
	}
	return accountID, nil
}
