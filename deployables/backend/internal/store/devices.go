package store

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
)

type Device struct {
	ID        int64
	AccountID int64
	Name      string
	PqAlg     string
	PublicKey []byte
	CreatedAt time.Time
	LastSeen  *time.Time
}

func (s *Store) CreateDevice(ctx context.Context, accountID int64, name, pqAlg string, publicKey []byte) (int64, error) {
	var id int64
	err := s.pool.QueryRow(ctx,
		`INSERT INTO devices (account_id, name, pq_alg, public_key)
		 VALUES ($1, $2, $3, $4)
		 RETURNING id`,
		accountID, name, pqAlg, publicKey,
	).Scan(&id)
	return id, err
}

func (s *Store) GetDevice(ctx context.Context, id int64) (Device, error) {
	var d Device
	err := s.pool.QueryRow(ctx,
		`SELECT id, account_id, name, pq_alg, public_key, created_at, last_seen
		 FROM devices
		 WHERE id = $1`,
		id,
	).Scan(&d.ID, &d.AccountID, &d.Name, &d.PqAlg, &d.PublicKey, &d.CreatedAt, &d.LastSeen)
	if errors.Is(err, pgx.ErrNoRows) {
		return Device{}, ErrNotFound
	}
	return d, err
}

func (s *Store) TouchDeviceLastSeen(ctx context.Context, id int64) error {
	_, err := s.pool.Exec(ctx,
		`UPDATE devices
		 SET last_seen = now()
		 WHERE id = $1`,
		id,
	)
	return err
}

func (s *Store) ListDevices(ctx context.Context, accountID int64) ([]Device, error) {
	rows, err := s.pool.Query(ctx,
		`SELECT id, account_id, name, pq_alg, public_key, created_at, last_seen
		 FROM devices
		 WHERE account_id = $1
		 ORDER BY created_at`,
		accountID,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	out := []Device{}
	for rows.Next() {
		var d Device
		if err := rows.Scan(&d.ID, &d.AccountID, &d.Name, &d.PqAlg, &d.PublicKey, &d.CreatedAt, &d.LastSeen); err != nil {
			return nil, err
		}
		out = append(out, d)
	}
	return out, rows.Err()
}

func (s *Store) RenameDevice(ctx context.Context, accountID, id int64, name string) error {
	tag, err := s.pool.Exec(ctx,
		`UPDATE devices SET name = $3 WHERE id = $1 AND account_id = $2`,
		id, accountID, name,
	)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// deleteDevice takes its sessions and open challanges with it (fk cascade)
func (s *Store) DeleteDevice(ctx context.Context, accountID, id int64) error {
	tag, err := s.pool.Exec(ctx,
		`DELETE FROM devices WHERE id = $1 AND account_id = $2`,
		id, accountID,
	)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}
