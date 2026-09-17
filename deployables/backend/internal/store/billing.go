package store

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
)

type UsageEvent struct {
	Kind             string // embed | rerank | storage
	Model            string
	Backend          string
	Quantity         int64
	Unit             string // tokens | mb_hour
	CostMicrocredits int64
}

// per kind, model and unit
type UsageRow struct {
	Kind             string
	Model            string
	Unit             string
	Calls            int64
	Quantity         int64
	CostMicrocredits int64
}

func (s *Store) Balance(ctx context.Context, accountID int64) (int64, error) {
	var balance int64
	err := s.pool.QueryRow(ctx,
		`SELECT balance_microcredits FROM accounts WHERE id = $1`,
		accountID,
	).Scan(&balance)
	return balance, err
}

// deduct + book in one tx. may go negative, the call already happened; billing.Precheck stops the next one
func (s *Store) Charge(ctx context.Context, accountID int64, ev UsageEvent) (int64, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return 0, err
	}
	defer tx.Rollback(ctx) //nolint:errcheck // no-op after commit

	balance, err := chargeTx(ctx, tx, accountID, ev)
	if err != nil {
		return 0, err
	}
	return balance, tx.Commit(ctx)
}

func chargeTx(ctx context.Context, tx pgx.Tx, accountID int64, ev UsageEvent) (int64, error) {
	var balance int64
	err := tx.QueryRow(ctx,
		`UPDATE accounts
		 SET balance_microcredits = balance_microcredits - $2
		 WHERE id = $1
		 RETURNING balance_microcredits`,
		accountID, ev.CostMicrocredits,
	).Scan(&balance)
	if err != nil {
		return 0, err
	}
	_, err = tx.Exec(ctx,
		`INSERT INTO usage_events (account_id, kind, model, backend, quantity, unit, cost_microcredits)
		 VALUES ($1, $2, $3, $4, $5, $6, $7)`,
		accountID, ev.Kind, ev.Model, ev.Backend, ev.Quantity, ev.Unit, ev.CostMicrocredits,
	)
	return balance, err
}

func (s *Store) AddCredits(ctx context.Context, accountID, amount int64) (int64, error) {
	var balance int64
	err := s.pool.QueryRow(ctx,
		`UPDATE accounts
		 SET balance_microcredits = balance_microcredits + $2
		 WHERE id = $1
		 RETURNING balance_microcredits`,
		accountID, amount,
	).Scan(&balance)
	return balance, err
}

func (s *Store) UsageSummary(ctx context.Context, accountID int64, since time.Time) ([]UsageRow, error) {
	rows, err := s.pool.Query(ctx,
		`SELECT kind, model, unit, count(*), sum(quantity), sum(cost_microcredits)
		 FROM usage_events
		 WHERE account_id = $1 AND created_at >= $2
		 GROUP BY kind, model, unit
		 ORDER BY kind, model, unit`,
		accountID, since,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	out := []UsageRow{}
	for rows.Next() {
		var r UsageRow
		if err := rows.Scan(&r.Kind, &r.Model, &r.Unit, &r.Calls, &r.Quantity, &r.CostMicrocredits); err != nil {
			return nil, err
		}
		out = append(out, r)
	}
	return out, rows.Err()
}

// --- storage billing ---

const mb = int64(1) << 20

// StoragePricer is billing.Pricing; the store needs it because storage is settled inside the backup transactions
type StoragePricer interface {
	StorageCost(bytes int64, d time.Duration) int64
}

func (s *Store) StorageUsed(ctx context.Context, accountID int64) (int64, error) {
	var bytes int64
	err := s.pool.QueryRow(ctx,
		`SELECT coalesce(sum(size_bytes), 0) FROM backups WHERE account_id = $1`,
		accountID,
	).Scan(&bytes)
	return bytes, err
}

// storageBillingDue lists accounts whose billing mark is older than minAge, plus never marked ones that store something
func (s *Store) StorageBillingDue(ctx context.Context, minAge time.Duration) ([]int64, error) {
	rows, err := s.pool.Query(ctx,
		`SELECT id FROM accounts a
		 WHERE (storage_billed_at IS NULL OR storage_billed_at < now() - make_interval(secs => $1))
		   AND (storage_billed_at IS NOT NULL OR EXISTS (SELECT 1 FROM backups b WHERE b.account_id = a.id))`,
		minAge.Seconds(),
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	ids := []int64{}
	for rows.Next() {
		var id int64
		if err := rows.Scan(&id); err != nil {
			return nil, err
		}
		ids = append(ids, id)
	}
	return ids, rows.Err()
}

// settleStorage bills the bytes stored right now for the time since the last mark
// and moves the mark. own tx under the account lock; the hourly loop uses this
func (s *Store) SettleStorage(ctx context.Context, accountID int64, pricer StoragePricer, now time.Time) (int64, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return 0, err
	}
	defer tx.Rollback(ctx) //nolint:errcheck // no-op after commit

	if err := lockAccount(ctx, tx, accountID); err != nil {
		return 0, err
	}
	_, cost, err := settleStorageTx(ctx, tx, accountID, pricer, now)
	if err != nil {
		return 0, err
	}
	return cost, tx.Commit(ctx)
}

func lockAccount(ctx context.Context, tx pgx.Tx, accountID int64) error {
	var id int64
	err := tx.QueryRow(ctx, `SELECT id FROM accounts WHERE id = $1 FOR UPDATE`, accountID).Scan(&id)
	if errors.Is(err, pgx.ErrNoRows) {
		return ErrNotFound
	}
	return err
}

// settleStorageTx runs before any change to the byte count so the old amount is paid up to now.
// caller holds the account lock. returns the bytes stored before the change
func settleStorageTx(ctx context.Context, tx pgx.Tx, accountID int64, pricer StoragePricer, now time.Time) (used, cost int64, err error) {
	var billedAt *time.Time
	if err := tx.QueryRow(ctx, `SELECT storage_billed_at FROM accounts WHERE id = $1`, accountID).Scan(&billedAt); err != nil {
		return 0, 0, err
	}
	if err := tx.QueryRow(ctx, `SELECT coalesce(sum(size_bytes), 0) FROM backups WHERE account_id = $1`, accountID).Scan(&used); err != nil {
		return 0, 0, err
	}
	if billedAt != nil {
		elapsed := now.Sub(*billedAt)
		if cost = pricer.StorageCost(used, elapsed); cost > 0 {
			_, err := chargeTx(ctx, tx, accountID, UsageEvent{
				Kind:             "storage",
				Model:            "backup",
				Backend:          "disk",
				Quantity:         int64(float64(used/mb) * elapsed.Hours()),
				Unit:             "mb_hour",
				CostMicrocredits: cost,
			})
			if err != nil {
				return 0, 0, err
			}
		}
	}
	_, err = tx.Exec(ctx, `UPDATE accounts SET storage_billed_at = $2 WHERE id = $1`, accountID, now)
	return used, cost, err
}
