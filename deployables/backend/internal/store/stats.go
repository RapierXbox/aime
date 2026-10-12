package store

import (
	"context"

	"github.com/jackc/pgx/v5/pgxpool"
)

type TableCounts struct {
	Accounts, Devices, ActiveSessions, Backups, BackupBytes int64
}

// one round trip, cheap enough for a 5 minute tick while the tables are small
func (s *Store) TableCounts(ctx context.Context) (TableCounts, error) {
	var c TableCounts
	err := s.pool.QueryRow(ctx,
		`SELECT (SELECT count(*) FROM accounts),
		        (SELECT count(*) FROM devices),
		        (SELECT count(*) FROM sessions WHERE expires_at > now()),
		        (SELECT count(*) FROM backups),
		        (SELECT coalesce(sum(size_bytes), 0) FROM backups)`,
	).Scan(&c.Accounts, &c.Devices, &c.ActiveSessions, &c.Backups, &c.BackupBytes)
	return c, err
}

func (s *Store) PoolStat() *pgxpool.Stat { return s.pool.Stat() }
