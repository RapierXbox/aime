package store

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

var (
	ErrBackupLimit    = errors.New("backup limit reached")
	ErrBackupInUse    = errors.New("backup is the parent of a delta")
	ErrBackupStale    = errors.New("parent is not the latest backup")
	ErrBackupBoundary = errors.New("prune boundary must be a full backup")
	ErrStorageQuota   = errors.New("free storage exceeded on empty balance")
	ErrStorageFull    = errors.New("server storage cap reached")
)

const (
	BackupFull  = "full"
	BackupDelta = "delta" // diff against ParentID, computed by the client
)

type Backup struct {
	ID          int64
	AccountID   int64
	Version     int
	Kind        string
	ParentID    *int64
	StoragePath string // relative to the blob root
	SizeBytes   int64
	Checksum    string // hex sha256 of the stored bytes
	Meta        []byte // opaque json from the client, stored byte for byte
	CreatedAt   time.Time
}

const backupCols = `id, account_id, version, kind, parent_id, storage_path, size_bytes, checksum, meta, created_at`

func scanBackup(row pgx.Row) (Backup, error) {
	var b Backup
	err := row.Scan(&b.ID, &b.AccountID, &b.Version, &b.Kind, &b.ParentID, &b.StoragePath, &b.SizeBytes, &b.Checksum, &b.Meta, &b.CreatedAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return Backup{}, ErrNotFound
	}
	return b, err
}

func collectBackups(rows pgx.Rows) ([]Backup, error) {
	defer rows.Close()
	out := []Backup{}
	for rows.Next() {
		b, err := scanBackup(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, b)
	}
	return out, rows.Err()
}

func (s *Store) ListBackups(ctx context.Context, accountID int64) ([]Backup, error) {
	rows, err := s.pool.Query(ctx,
		`SELECT `+backupCols+` FROM backups WHERE account_id = $1 ORDER BY version DESC`,
		accountID,
	)
	if err != nil {
		return nil, err
	}
	return collectBackups(rows)
}

func (s *Store) CountBackups(ctx context.Context, accountID int64) (int, error) {
	var n int
	err := s.pool.QueryRow(ctx, `SELECT count(*) FROM backups WHERE account_id = $1`, accountID).Scan(&n)
	return n, err
}

// every query is scoped by account, so one account can never see another ones rows
func (s *Store) GetBackup(ctx context.Context, accountID, id int64) (Backup, error) {
	return scanBackup(s.pool.QueryRow(ctx,
		`SELECT `+backupCols+` FROM backups WHERE id = $1 AND account_id = $2`,
		id, accountID,
	))
}

type NewBackup struct {
	ParentID    *int64 // set for a delta; must be the accounts current head
	StoragePath string
	SizeBytes   int64
	Checksum    string
	Meta        []byte
}

// Quota is what CreateBackup enforces under the account lock, where it cannot be raced
type Quota struct {
	MaxVersions   int
	FreeBytes     int64 // per account before a positive balance is required
	Enforce       bool  // BILLING_ENFORCE
	TotalCapBytes int64 // whole server, 0 = unlimited
}

// createBackup does everything that must be atomic per account: bill the old byte
// count up to now, check versions/quota/cap, take the next version from the counter
// (never reused, unlike max+1), verify a delta builds on the head, insert
func (s *Store) CreateBackup(ctx context.Context, accountID int64, q Quota, pricer StoragePricer, nb NewBackup) (Backup, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return Backup{}, err
	}
	defer tx.Rollback(ctx) //nolint:errcheck // no-op after commit

	// the update takes the row lock and hands out the version in one step
	var version int
	var balance int64
	err = tx.QueryRow(ctx,
		`UPDATE accounts SET backup_version_seq = backup_version_seq + 1
		 WHERE id = $1
		 RETURNING backup_version_seq, balance_microcredits`,
		accountID,
	).Scan(&version, &balance)
	if errors.Is(err, pgx.ErrNoRows) {
		return Backup{}, ErrNotFound
	}
	if err != nil {
		return Backup{}, err
	}

	used, _, err := settleStorageTx(ctx, tx, accountID, pricer, time.Now())
	if err != nil {
		return Backup{}, err
	}

	var count, headVersion int
	err = tx.QueryRow(ctx,
		`SELECT count(*), coalesce(max(version), 0) FROM backups WHERE account_id = $1`,
		accountID,
	).Scan(&count, &headVersion)
	if err != nil {
		return Backup{}, err
	}
	if count >= q.MaxVersions {
		return Backup{}, ErrBackupLimit
	}
	if q.Enforce && balance <= 0 && used+nb.SizeBytes > q.FreeBytes {
		return Backup{}, ErrStorageQuota
	}
	if q.TotalCapBytes > 0 {
		var total int64
		if err := tx.QueryRow(ctx, `SELECT coalesce(sum(size_bytes), 0) FROM backups`).Scan(&total); err != nil {
			return Backup{}, err
		}
		if total+nb.SizeBytes > q.TotalCapBytes {
			return Backup{}, ErrStorageFull
		}
	}

	kind := BackupFull
	if nb.ParentID != nil {
		kind = BackupDelta
		var parentVersion int
		err = tx.QueryRow(ctx, `SELECT version FROM backups WHERE id = $1 AND account_id = $2`, *nb.ParentID, accountID).Scan(&parentVersion)
		if errors.Is(err, pgx.ErrNoRows) {
			return Backup{}, ErrNotFound
		}
		if err != nil {
			return Backup{}, err
		}
		if parentVersion != headVersion {
			return Backup{}, ErrBackupStale
		}
	}

	b, err := scanBackup(tx.QueryRow(ctx,
		`INSERT INTO backups (account_id, version, kind, parent_id, storage_path, size_bytes, checksum, meta)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
		 RETURNING `+backupCols,
		accountID, version, kind, nb.ParentID, nb.StoragePath, nb.SizeBytes, nb.Checksum, string(nb.Meta),
	))
	if err != nil {
		return Backup{}, err
	}
	return b, tx.Commit(ctx)
}

// deleteBackup settles storage first (the bytes were stored until now), then removes
// the row and returns it so the caller can remove the file.
// a base that still has a delta pointing at it comes back as ErrBackupInUse
func (s *Store) DeleteBackup(ctx context.Context, accountID, id int64, pricer StoragePricer) (Backup, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return Backup{}, err
	}
	defer tx.Rollback(ctx) //nolint:errcheck // no-op after commit

	if err := lockAccount(ctx, tx, accountID); err != nil {
		return Backup{}, err
	}
	if _, _, err := settleStorageTx(ctx, tx, accountID, pricer, time.Now()); err != nil {
		return Backup{}, err
	}

	b, err := scanBackup(tx.QueryRow(ctx,
		`DELETE FROM backups WHERE id = $1 AND account_id = $2 RETURNING `+backupCols,
		id, accountID,
	))
	var pgErr *pgconn.PgError
	if errors.As(err, &pgErr) && pgErr.Code == "23503" { // foreign_key_violation from parent_id
		return Backup{}, ErrBackupInUse
	}
	if err != nil {
		return Backup{}, err
	}
	return b, tx.Commit(ctx)
}

// pruneBackups deletes every version below beforeVersion and returns the rows.
// a delta always builds on the version right before it (head rule), so the kept
// chain is self contained exactly when beforeVersion itself is a full backup
func (s *Store) PruneBackups(ctx context.Context, accountID int64, beforeVersion int, pricer StoragePricer) ([]Backup, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return nil, err
	}
	defer tx.Rollback(ctx) //nolint:errcheck // no-op after commit

	if err := lockAccount(ctx, tx, accountID); err != nil {
		return nil, err
	}
	if _, _, err := settleStorageTx(ctx, tx, accountID, pricer, time.Now()); err != nil {
		return nil, err
	}

	var kind string
	err = tx.QueryRow(ctx,
		`SELECT kind FROM backups WHERE account_id = $1 AND version = $2`,
		accountID, beforeVersion,
	).Scan(&kind)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, err
	}
	if kind != BackupFull {
		return nil, ErrBackupBoundary
	}

	rows, err := tx.Query(ctx,
		`DELETE FROM backups WHERE account_id = $1 AND version < $2 RETURNING `+backupCols,
		accountID, beforeVersion,
	)
	if err != nil {
		return nil, err
	}
	out, err := collectBackups(rows)
	if err != nil {
		return nil, err
	}
	return out, tx.Commit(ctx)
}

// allStoragePaths feeds the orphan sweep
func (s *Store) AllStoragePaths(ctx context.Context) (map[string]struct{}, error) {
	rows, err := s.pool.Query(ctx, `SELECT storage_path FROM backups`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	set := map[string]struct{}{}
	for rows.Next() {
		var p string
		if err := rows.Scan(&p); err != nil {
			return nil, err
		}
		set[p] = struct{}{}
	}
	return set, rows.Err()
}
