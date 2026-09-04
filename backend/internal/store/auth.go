package store

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
)

// errchallangeinvalid covers every reson a challange cannot be redeemed
// since differences between already used or expired can be used by a attacker
var ErrChallengeInvalid = errors.New("challenge invalid or expired")

type Challenge struct {
	ID       int64
	DeviceID int64
	Nonce    []byte
}

// createChallange stores a fresh nonce for a device and returns its id
func (s *Store) CreateChallenge(ctx context.Context, deviceID int64, nonce []byte, ttl time.Duration) (int64, error) {
	var id int64
	err := s.pool.QueryRow(ctx,
		`INSERT INTO auth_challanges (device_id, nonce, expires_at)
		 VALUES ($1, $2, now() + make_interval(secs => $3))
		 RETURNING id`,
		deviceID, nonce, ttl.Seconds(),
	).Scan(&id)
	return id, err
}

// consumeChallange redemms a challange exactly once and returns it
// a second call with same id must fail even if races with the first
func (s *Store) ConsumeChallenge(ctx context.Context, id int64) (Challenge, error) {
	var deviceID int64
	var nonce []byte
	err := s.pool.QueryRow(ctx,
		`UPDATE auth_challanges
		 SET used = true
		 WHERE id = $1 AND NOT used AND expires_at > now()
		 RETURNING device_id, nonce`,
		id,
	).Scan(&deviceID, &nonce)
	if errors.Is(err, pgx.ErrNoRows) {
		return Challenge{}, ErrChallengeInvalid
	}
	if err != nil {
		return Challenge{}, err
	}
	return Challenge{ID: id, DeviceID: deviceID, Nonce: nonce}, nil
}
