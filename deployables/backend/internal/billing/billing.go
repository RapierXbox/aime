// package billing turns usage into credits and keeps the account ledger.
package billing

import (
	"context"
	"errors"
	"log/slog"
	"math"
	"time"

	"github.com/rapierxbox/aime/backend/internal/encode"
	"github.com/rapierxbox/aime/backend/internal/store"
)

var ErrInsufficientCredits = errors.New("insufficient credits")

const (
	mb    = int64(1) << 20
	gb    = int64(1) << 30
	month = 30 * 24 * time.Hour

	storageBillEvery = 24 * time.Hour // per account; the elapsed time is prorated exactly
)

// microcredits; 1 credit = 1_000_000
type Pricing struct {
	EmbedPer1kTokens  int64
	RerankPer1kTokens int64
	StoragePerGBMonth int64 // for stored bytes above FreeStorageMB, prorated per day
	FreeStorageMB     int64 // per account, all backups together
}

var DefaultPricing = Pricing{
	EmbedPer1kTokens:  20_000,
	RerankPer1kTokens: 50_000,
	StoragePerGBMonth: 500_000,
	FreeStorageMB:     500,
}

// cost rounds up so no call is free
func (p Pricing) Cost(kind encode.Kind, inputTokens int) int64 {
	var per1k int64
	switch kind {
	case encode.KindEmbed:
		per1k = p.EmbedPer1kTokens
	case encode.KindRerank:
		per1k = p.RerankPer1kTokens
	default:
		return 0
	}
	if inputTokens <= 0 {
		return 0
	}
	return (int64(inputTokens)*per1k + 999) / 1000
}

func (p Pricing) StorageBillable(bytes int64) bool { return bytes > p.FreeStorageMB*mb }

// storageCost for keeping `bytes` stored for `d`, only the part above the free quota counts.
// float64 is fine here: values stay far below 2^53 and the result is rounded up to whole microcredits
func (p Pricing) StorageCost(bytes int64, d time.Duration) int64 {
	over := bytes - p.FreeStorageMB*mb
	if over <= 0 || d <= 0 {
		return 0
	}
	gbMonths := float64(over) / float64(gb) * (float64(d) / float64(month))
	return int64(math.Ceil(gbMonths * float64(p.StoragePerGBMonth)))
}

// receipt is what the client gets to see about a charge
type Receipt struct {
	CostMicrocredits    int64
	BalanceMicrocredits int64
}

// Biller is what the http handlers need
type Biller interface {
	Precheck(ctx context.Context, accountID int64) error
	Charge(ctx context.Context, accountID int64, kind encode.Kind, info encode.Info, usage encode.Usage) (Receipt, error)
	StorageBillable(bytes int64) bool
	FreeStorageBytes() int64
	StorageCost(bytes int64, d time.Duration) int64 // store.StoragePricer
}

// Ledger bills against the accounts table
type Ledger struct {
	store   *store.Store
	pricing Pricing
	enforce bool // false: precheck always passes (dev), usage is still booked
}

func NewLedger(st *store.Store, pricing Pricing, enforce bool) *Ledger {
	return &Ledger{store: st, pricing: pricing, enforce: enforce}
}

func (l *Ledger) Precheck(ctx context.Context, accountID int64) error {
	if !l.enforce {
		return nil
	}
	balance, err := l.store.Balance(ctx, accountID)
	if err != nil {
		return err
	}
	if balance <= 0 {
		return ErrInsufficientCredits
	}
	return nil
}

func (l *Ledger) Charge(ctx context.Context, accountID int64, kind encode.Kind, info encode.Info, usage encode.Usage) (Receipt, error) {
	model := info.EmbedModel
	if kind == encode.KindRerank {
		model = info.RerankModel
	}
	cost := l.pricing.Cost(kind, usage.InputTokens)
	balance, err := l.store.Charge(ctx, accountID, store.UsageEvent{
		Kind:             string(kind),
		Model:            model,
		Backend:          info.Backend,
		Quantity:         int64(usage.InputTokens),
		Unit:             "tokens",
		CostMicrocredits: cost,
	})
	if err != nil {
		return Receipt{}, err
	}
	return Receipt{CostMicrocredits: cost, BalanceMicrocredits: balance}, nil
}

func (l *Ledger) StorageBillable(bytes int64) bool { return l.pricing.StorageBillable(bytes) }

func (l *Ledger) FreeStorageBytes() int64 { return l.pricing.FreeStorageMB * mb }

func (l *Ledger) StorageCost(bytes int64, d time.Duration) int64 {
	return l.pricing.StorageCost(bytes, d)
}

type StorageBillingReport struct {
	Accounts     int
	Microcredits int64
}

// BillStorage settles every account whose mark is a day old. uploads and deletes
// settle on their own (store.settleStorageTx), so this only catches idle accounts
// and nobody can dodge a period by deleting right before the tick
func (l *Ledger) BillStorage(ctx context.Context, now time.Time, log *slog.Logger) (StorageBillingReport, error) {
	var rep StorageBillingReport
	due, err := l.store.StorageBillingDue(ctx, storageBillEvery)
	if err != nil {
		return rep, err
	}
	for _, id := range due {
		cost, err := l.store.SettleStorage(ctx, id, l.pricing, now)
		if err != nil {
			return rep, err
		}
		rep.Accounts++
		rep.Microcredits += cost
		if cost > 0 {
			log.Info("storage billed", "account_id", id, "cost_microcredits", cost)
		}
	}
	return rep, nil
}
