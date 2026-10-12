package metrics

import (
	"net/http"
	"strconv"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/collectors"
	"github.com/prometheus/client_golang/prometheus/promhttp"
)

// metrics owns a private registry so that /metrics only exposes what we put there rater then whatever dependency registered on the global default
type Metrics struct {
	reg *prometheus.Registry

	// http
	httpDuration *prometheus.HistogramVec // method, route, status
	httpInflight prometheus.Gauge
	httpReqBytes *prometheus.HistogramVec // route
	httpResBytes *prometheus.HistogramVec // route
	rateLimited  *prometheus.CounterVec   // route

	// inference
	inferDuration  *prometheus.HistogramVec // kind, backend, outcome
	inferQueueWait *prometheus.HistogramVec // kind
	inferInflight  prometheus.Gauge
	inferTokens    *prometheus.CounterVec   // kind
	inferBatch     *prometheus.HistogramVec // kind

	// billing
	billingCharged  *prometheus.CounterVec // kind
	billingRejected *prometheus.CounterVec // kind
	billingFailed   *prometheus.CounterVec // kind

	// auth + background loops
	authEvents          *prometheus.CounterVec // event
	janitorPurged       *prometheus.CounterVec // table
	storageBillAccounts prometheus.Counter
	storageBillCredits  prometheus.Counter

	// backups
	backupOps        *prometheus.CounterVec // op, result
	backupBytes      *prometheus.CounterVec // direction
	backupSweptFiles prometheus.Counter
	backupSweptBytes prometheus.Counter

	// table counts, refreshed by the stats loop
	accounts, devices, sessions, backups, backupBytesStored prometheus.Gauge
}

var (
	bytesBuckets = prometheus.ExponentialBuckets(256, 4, 12) // 256B .. 1GiB
	batchBuckets = []float64{1, 2, 4, 8, 16, 32, 64}
	inferBuckets = []float64{.01, .025, .05, .1, .25, .5, 1, 2.5, 5, 10, 30}
)

func New() *Metrics {
	m := &Metrics{reg: prometheus.NewRegistry()}
	hist := func(sub, name, help string, buckets []float64, labels ...string) *prometheus.HistogramVec {
		return prometheus.NewHistogramVec(prometheus.HistogramOpts{Namespace: "aime", Subsystem: sub, Name: name, Help: help, Buckets: buckets}, labels)
	}
	counter := func(sub, name, help string, labels ...string) *prometheus.CounterVec {
		return prometheus.NewCounterVec(prometheus.CounterOpts{Namespace: "aime", Subsystem: sub, Name: name, Help: help}, labels)
	}
	gauge := func(sub, name, help string) prometheus.Gauge {
		return prometheus.NewGauge(prometheus.GaugeOpts{Namespace: "aime", Subsystem: sub, Name: name, Help: help})
	}

	m.httpDuration = hist("http", "request_duration_seconds", "time to serve a request, by route pattern and status", prometheus.DefBuckets, "method", "route", "status")
	m.httpInflight = gauge("http", "inflight", "requests currently being served")
	m.httpReqBytes = hist("http", "request_bytes", "request body bytes read", bytesBuckets, "route")
	m.httpResBytes = hist("http", "response_bytes", "response body bytes written", bytesBuckets, "route")
	m.rateLimited = counter("http", "rate_limited_total", "requests rejected with 429", "route")

	m.inferDuration = hist("infer", "duration_seconds", "wall time of one model call including queueing", inferBuckets, "kind", "backend", "outcome")
	m.inferQueueWait = hist("infer", "queue_wait_seconds", "time spent waiting for a concurrency slot", inferBuckets, "kind")
	m.inferInflight = gauge("infer", "inflight", "model calls running right now")
	m.inferTokens = counter("infer", "tokens_total", "input tokens processed", "kind")
	m.inferBatch = hist("infer", "batch_size", "texts per request", batchBuckets, "kind")

	m.billingCharged = counter("billing", "charged_microcredits_total", "microcredits booked", "kind")
	m.billingRejected = counter("billing", "rejected_total", "requests refused with 402", "kind")
	m.billingFailed = counter("billing", "charge_failures_total", "served but not booked because the ledger write failed", "kind")

	m.authEvents = counter("auth", "events_total", "auth flow events", "event")
	m.janitorPurged = counter("janitor", "purged_total", "expired rows deleted", "table")
	m.storageBillAccounts = prometheus.NewCounter(prometheus.CounterOpts{Namespace: "aime", Subsystem: "storage_billing", Name: "accounts_total", Help: "accounts processed by the storage biller"})
	m.storageBillCredits = prometheus.NewCounter(prometheus.CounterOpts{Namespace: "aime", Subsystem: "storage_billing", Name: "microcredits_total", Help: "microcredits charged for storage"})

	m.backupOps = counter("backup", "ops_total", "backup operations", "op", "result")
	m.backupBytes = counter("backup", "transfer_bytes_total", "backup bytes moved", "direction")
	m.backupSweptFiles = prometheus.NewCounter(prometheus.CounterOpts{Namespace: "aime", Subsystem: "backup", Name: "swept_files_total", Help: "orphan files removed by the sweep"})
	m.backupSweptBytes = prometheus.NewCounter(prometheus.CounterOpts{Namespace: "aime", Subsystem: "backup", Name: "swept_bytes_total", Help: "orphan bytes removed by the sweep"})

	m.accounts = gauge("db", "accounts", "accounts")
	m.devices = gauge("db", "devices", "enrolled devices")
	m.sessions = gauge("db", "sessions_active", "unexpired sessions")
	m.backups = gauge("db", "backups", "backup rows")
	m.backupBytesStored = gauge("backup", "bytes_stored", "bytes of all backups on disk")

	m.reg.MustRegister(
		collectors.NewGoCollector(),
		collectors.NewProcessCollector(collectors.ProcessCollectorOpts{}),
		m.httpDuration, m.httpInflight, m.httpReqBytes, m.httpResBytes, m.rateLimited,
		m.inferDuration, m.inferQueueWait, m.inferInflight, m.inferTokens, m.inferBatch,
		m.billingCharged, m.billingRejected, m.billingFailed,
		m.authEvents, m.janitorPurged, m.storageBillAccounts, m.storageBillCredits,
		m.backupOps, m.backupBytes, m.backupSweptFiles, m.backupSweptBytes,
		m.accounts, m.devices, m.sessions, m.backups, m.backupBytesStored,
	)
	return m
}

// handler serves the registry for mouting at GET /metrics
func (m *Metrics) Handler() http.Handler {
	return promhttp.HandlerFor(m.reg, promhttp.HandlerOpts{})
}

// --- http ---

// route is the mux pattern, never the raw path (cardinality)
func (m *Metrics) ObserveHTTP(method, route string, status int, d time.Duration, reqBytes, resBytes int64) {
	m.httpDuration.WithLabelValues(method, route, strconv.Itoa(status)).Observe(d.Seconds())
	m.httpReqBytes.WithLabelValues(route).Observe(float64(reqBytes))
	m.httpResBytes.WithLabelValues(route).Observe(float64(resBytes))
}

func (m *Metrics) HTTPInflight(delta int) { m.httpInflight.Add(float64(delta)) }

func (m *Metrics) IncRateLimited(route string) { m.rateLimited.WithLabelValues(route).Inc() }

// --- inference ---

// outcome: ok | error | timeout
func (m *Metrics) ObserveInference(kind, backend, outcome string, d time.Duration) {
	m.inferDuration.WithLabelValues(kind, backend, outcome).Observe(d.Seconds())
}

func (m *Metrics) ObserveQueueWait(kind string, d time.Duration) {
	m.inferQueueWait.WithLabelValues(kind).Observe(d.Seconds())
}

func (m *Metrics) InferInflight(delta int) { m.inferInflight.Add(float64(delta)) }

func (m *Metrics) ObserveBatch(kind string, texts, tokens int) {
	m.inferBatch.WithLabelValues(kind).Observe(float64(texts))
	m.inferTokens.WithLabelValues(kind).Add(float64(tokens))
}

// --- billing ---

func (m *Metrics) AddCharged(kind string, microcredits int64) {
	m.billingCharged.WithLabelValues(kind).Add(float64(microcredits))
}

func (m *Metrics) IncRejected(kind string)      { m.billingRejected.WithLabelValues(kind).Inc() }
func (m *Metrics) IncChargeFailure(kind string) { m.billingFailed.WithLabelValues(kind).Inc() }

func (m *Metrics) AddStorageBilled(accounts int, microcredits int64) {
	m.storageBillAccounts.Add(float64(accounts))
	m.storageBillCredits.Add(float64(microcredits))
}

// --- auth + janitor ---

// event: signup | login_ok | login_fail | enroll | verify_ok | verify_fail | logout
func (m *Metrics) IncAuth(event string) { m.authEvents.WithLabelValues(event).Inc() }

func (m *Metrics) AddPurged(table string, n int64) {
	m.janitorPurged.WithLabelValues(table).Add(float64(n))
}

// --- backups ---

// op: upload | download | delete; result: ok | rejected | error
func (m *Metrics) IncBackupOp(op, result string) { m.backupOps.WithLabelValues(op, result).Inc() }

// direction: in | out
func (m *Metrics) AddBackupBytes(direction string, n int64) {
	m.backupBytes.WithLabelValues(direction).Add(float64(n))
}

func (m *Metrics) AddSwept(files int, bytes int64) {
	m.backupSweptFiles.Add(float64(files))
	m.backupSweptBytes.Add(float64(bytes))
}

// --- gauges fed from outside ---

type TableCounts struct {
	Accounts, Devices, ActiveSessions, Backups, BackupBytes int64
}

func (m *Metrics) SetTableCounts(c TableCounts) {
	m.accounts.Set(float64(c.Accounts))
	m.devices.Set(float64(c.Devices))
	m.sessions.Set(float64(c.ActiveSessions))
	m.backups.Set(float64(c.Backups))
	m.backupBytesStored.Set(float64(c.BackupBytes))
}

type DBPoolStats struct {
	Total, Idle, Acquired, Constructing int64
	AcquireCount, EmptyAcquireCount     int64
	AcquireWait                         time.Duration // cumulative
}

// registerDBPool reads the pool at scrape time
func (m *Metrics) RegisterDBPool(stat func() DBPoolStats) {
	conns := func(state string, pick func(DBPoolStats) int64) prometheus.GaugeFunc {
		return prometheus.NewGaugeFunc(prometheus.GaugeOpts{
			Namespace: "aime", Subsystem: "db", Name: "pool_conns", Help: "connections in the pgx pool",
			ConstLabels: prometheus.Labels{"state": state},
		}, func() float64 { return float64(pick(stat())) })
	}
	m.reg.MustRegister(
		conns("total", func(s DBPoolStats) int64 { return s.Total }),
		conns("idle", func(s DBPoolStats) int64 { return s.Idle }),
		conns("acquired", func(s DBPoolStats) int64 { return s.Acquired }),
		conns("constructing", func(s DBPoolStats) int64 { return s.Constructing }),
		prometheus.NewCounterFunc(prometheus.CounterOpts{Namespace: "aime", Subsystem: "db", Name: "pool_acquire_total", Help: "connection acquires"},
			func() float64 { return float64(stat().AcquireCount) }),
		prometheus.NewCounterFunc(prometheus.CounterOpts{Namespace: "aime", Subsystem: "db", Name: "pool_acquire_empty_total", Help: "acquires that had to wait for a connection"},
			func() float64 { return float64(stat().EmptyAcquireCount) }),
		prometheus.NewCounterFunc(prometheus.CounterOpts{Namespace: "aime", Subsystem: "db", Name: "pool_acquire_wait_seconds_total", Help: "cumulative time spent waiting for a connection"},
			func() float64 { return stat().AcquireWait.Seconds() }),
	)
}

// registerRateLimiter exposes how many ips a limiter currently tracks
func (m *Metrics) RegisterRateLimiter(name string, size func() int) {
	m.reg.MustRegister(prometheus.NewGaugeFunc(prometheus.GaugeOpts{
		Namespace: "aime", Subsystem: "ratelimit", Name: "tracked_ips", Help: "ips with a live token bucket",
		ConstLabels: prometheus.Labels{"limiter": name},
	}, func() float64 { return float64(size()) }))
}
