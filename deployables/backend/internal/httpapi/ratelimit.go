package httpapi

import (
	"net"
	"net/http"
	"strings"
	"sync"
	"time"

	"golang.org/x/time/rate"
)

// token bucket per client ip, idle entries evicted on the next call
type ipLimiter struct {
	limit rate.Limit
	burst int

	mu        sync.Mutex
	seen      map[string]*ipEntry
	lastSweep time.Time
}

type ipEntry struct {
	lim  *rate.Limiter
	last time.Time
}

const (
	evictAfter = 10 * time.Minute
	maxTracked = 10_000 // beyond this new ips are allowed untracked rather than growing the map
)

func newIPLimiter(limit rate.Limit, burst int) *ipLimiter {
	return &ipLimiter{limit: limit, burst: burst, seen: make(map[string]*ipEntry), lastSweep: time.Now()}
}

func (l *ipLimiter) size() int {
	l.mu.Lock()
	defer l.mu.Unlock()
	return len(l.seen)
}

func (l *ipLimiter) allow(ip string, now time.Time) bool {
	l.mu.Lock()
	defer l.mu.Unlock()

	if now.Sub(l.lastSweep) > evictAfter {
		for k, e := range l.seen {
			if now.Sub(e.last) > evictAfter {
				delete(l.seen, k)
			}
		}
		l.lastSweep = now
	}

	e, ok := l.seen[ip]
	if !ok {
		if len(l.seen) >= maxTracked {
			return true
		}
		e = &ipEntry{lim: rate.NewLimiter(l.limit, l.burst)}
		l.seen[ip] = e
	}
	e.last = now
	return e.lim.AllowN(now, 1)
}

func (s *Server) RateLimit(l *ipLimiter, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !l.allow(s.clientIP(r), time.Now()) {
			s.Metrics.IncRateLimited(routeLabel(r))
			w.Header().Set("Retry-After", "10")
			s.writeError(w, http.StatusTooManyRequests, "too many requests")
			return
		}
		next.ServeHTTP(w, r)
	})
}

// the last X-Forwarded-For hop is the only one a client cannot forge
func (s *Server) clientIP(r *http.Request) string {
	if s.Cfg.TrustProxy {
		if xff := r.Header.Get("X-Forwarded-For"); xff != "" {
			parts := strings.Split(xff, ",")
			if ip := net.ParseIP(strings.TrimSpace(parts[len(parts)-1])); ip != nil {
				return ip.String()
			}
		}
	}
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}
