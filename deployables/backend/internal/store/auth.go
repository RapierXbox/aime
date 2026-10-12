package store

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

// errchallangeinvalid covers every reson a challange cannot be redeemed
// since differences between already used or expired can be used by a attacker
var ErrChallengeInvalid = errors.New("challenge invalid or expired")

type Challenge struct {
	ID       int64
	DeviceID int64
	Nonce    []byte
}

type Session struct {
	AccountID int64
	DeviceID  int64
	Expires   time.Time
}

// createChallange stores a fresh nonce for a device and returns its id. unknown device -> ErrNotFound
func (s *Store) CreateChallenge(ctx context.Context, deviceID int64, nonce []byte, ttl time.Duration) (int64, error) {
	var id int64
	err := s.pool.QueryRow(ctx,
		`INSERT INTO auth_challanges (device_id, nonce, expires_at)
		 VALUES ($1, $2, now() + make_interval(secs => $3))
		 RETURNING id`,
		deviceID, nonce, ttl.Seconds(),
	).Scan(&id)

	var pgErr *pgconn.PgError
	if errors.As(err, &pgErr) && pgErr.Code == "23503" { // foreign_key_violation
		return 0, ErrNotFound
	}
	return id, err
}

// consumeChallange redemms a challange exactly once and returns it
// a second call with same id must fail even if races with the first.
// ids are sequential, so the caller must also present the nonce it was given
func (s *Store) ConsumeChallenge(ctx context.Context, id int64, nonce []byte) (Challenge, error) {
	var deviceID int64
	err := s.pool.QueryRow(ctx,
		`UPDATE auth_challanges
		 SET used = true
		 WHERE id = $1 AND nonce = $2 AND NOT used AND expires_at > now()
		 RETURNING device_id`,
		id, nonce,
	).Scan(&deviceID)
	if errors.Is(err, pgx.ErrNoRows) {
		return Challenge{}, ErrChallengeInvalid
	}
	if err != nil {
		return Challenge{}, err
	}
	return Challenge{ID: id, DeviceID: deviceID, Nonce: nonce}, nil
}

func (s *Store) CreateSession(ctx context.Context, accountID, deviceID int64, tokenHash []byte, ttl time.Duration) error {
	_, err := s.pool.Exec(ctx,
		`INSERT INTO sessions (account_id, device_id, token_hash, expires_at)
		 VALUES ($1, $2, $3, now() + make_interval(secs => $4))`,
		accountID, deviceID, tokenHash, ttl.Seconds(),
	)
	return err
}

func (s *Store) SessionByTokenHash(ctx context.Context, tokenHash []byte) (Session, error) {
	var session Session
	err := s.pool.QueryRow(ctx,
		`SELECT account_id, device_id, expires_at
		 FROM sessions
		 WHERE token_hash = $1 AND expires_at > now()`,
		tokenHash,
	).Scan(&session.AccountID, &session.DeviceID, &session.Expires)
	if errors.Is(err, pgx.ErrNoRows) {
		return Session{}, ErrNotFound
	}
	return session, err
}

// logout; an already gone token is fine
func (s *Store) DeleteSession(ctx context.Context, tokenHash []byte) error {
	_, err := s.pool.Exec(ctx, `DELETE FROM sessions WHERE token_hash = $1`, tokenHash)
	return err
}

// deleteSessions revokes every session of the account; keep (a token hash) survives if not nil
func (s *Store) DeleteSessions(ctx context.Context, accountID int64, keep []byte) (int64, error) {
	tag, err := s.pool.Exec(ctx,
		`DELETE FROM sessions WHERE account_id = $1 AND ($2::bytea IS NULL OR token_hash <> $2)`,
		accountID, keep,
	)
	if err != nil {
		return 0, err
	}
	return tag.RowsAffected(), nil
}
