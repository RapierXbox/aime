package httpapi

import (
	"context"
	"errors"
	"net/http"
	"time"

	"github.com/rapierxbox/aime/backend/internal/billing"
	"github.com/rapierxbox/aime/backend/internal/encode"
)

const (
	maxInferBody  = 4 << 20 // ~2MiB worst case input + json headroom
	chargeTimeout = 5 * time.Second
)

// runInference: precheck -> slot + timeout -> metric -> charge. the only place a model gets called.
// texts is the batch size, only used for metrics
func (s *Server) runInference(ctx context.Context, accountID int64, kind encode.Kind, texts int, fn func(ctx context.Context) (encode.Usage, error)) (billing.Receipt, error) {
	if err := s.Billing.Precheck(ctx, accountID); err != nil {
		if errors.Is(err, billing.ErrInsufficientCredits) {
			s.Metrics.IncRejected(string(kind))
		}
		return billing.Receipt{}, err
	}

	inferCtx, cancel := context.WithTimeout(ctx, s.Cfg.InferTimeout)
	defer cancel()

	start := time.Now()
	usage, err := s.withInferSlot(inferCtx, kind, fn)
	s.Metrics.ObserveInference(string(kind), s.Encoder.Info().Backend, inferOutcome(err), time.Since(start))
	if err != nil {
		return billing.Receipt{}, err
	}
	s.Metrics.ObserveBatch(string(kind), texts, usage.InputTokens)

	// charge even if the client disconnected
	chargeCtx, cancelCharge := context.WithTimeout(context.WithoutCancel(ctx), chargeTimeout)
	defer cancelCharge()
	receipt, err := s.Billing.Charge(chargeCtx, accountID, kind, s.Encoder.Info(), usage)
	if err != nil {
		// result still goes out, a 500 would just make them redo the compute
		s.Metrics.IncChargeFailure(string(kind))
		s.Log.Error("charging failed, usage not booked", "error", err, "account_id", accountID, "kind", kind, "input_tokens", usage.InputTokens)
		return billing.Receipt{}, nil
	}
	s.Metrics.AddCharged(string(kind), receipt.CostMicrocredits)
	return receipt, nil
}

func (s *Server) withInferSlot(ctx context.Context, kind encode.Kind, fn func(ctx context.Context) (encode.Usage, error)) (encode.Usage, error) {
	wait := time.Now()
	select {
	case s.inferSem <- struct{}{}:
		s.Metrics.ObserveQueueWait(string(kind), time.Since(wait))
		s.Metrics.InferInflight(1)
		defer func() {
			<-s.inferSem
			s.Metrics.InferInflight(-1)
		}()
		return fn(ctx)
	case <-ctx.Done():
		s.Metrics.ObserveQueueWait(string(kind), time.Since(wait))
		return encode.Usage{}, ctx.Err()
	}
}

func inferOutcome(err error) string {
	switch {
	case err == nil:
		return "ok"
	case errors.Is(err, context.DeadlineExceeded):
		return "timeout"
	default:
		return "error"
	}
}

func (s *Server) writeInferError(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, context.Canceled):
		// client gone
	case errors.Is(err, billing.ErrInsufficientCredits):
		s.writeError(w, http.StatusPaymentRequired, "insufficient credits")
	case errors.Is(err, context.DeadlineExceeded):
		s.writeError(w, http.StatusGatewayTimeout, "inference timeout")
	case errors.Is(err, encode.ErrNotImplemented):
		s.writeError(w, http.StatusNotImplemented, "backend not available")
	default:
		s.Log.Error("inference failed", "error", err)
		s.writeError(w, http.StatusInternalServerError, "internal")
	}
}

type usageRes struct {
	InputTokens         int   `json:"input_tokens"`
	CostMicrocredits    int64 `json:"cost_microcredits"`
	BalanceMicrocredits int64 `json:"balance_microcredits"`
}

func newUsageRes(usage encode.Usage, receipt billing.Receipt) usageRes {
	return usageRes{
		InputTokens:         usage.InputTokens,
		CostMicrocredits:    receipt.CostMicrocredits,
		BalanceMicrocredits: receipt.BalanceMicrocredits,
	}
}
