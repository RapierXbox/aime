package store

import "context"

type Purged struct {
	Challenges       int64
	EnrollmentTokens int64
	Sessions         int64
}

func (p Purged) Total() int64 { return p.Challenges + p.EnrollmentTokens + p.Sessions }

// challanges and enrollment tokens come from unauthenticated routes, without this they grow forever
func (s *Store) PurgeExpired(ctx context.Context) (Purged, error) {
	var p Purged
	for _, q := range []struct {
		table string
		out   *int64
	}{
		{"auth_challanges", &p.Challenges},
		{"enrollment_tokens", &p.EnrollmentTokens},
		{"sessions", &p.Sessions},
	} {
		tag, err := s.pool.Exec(ctx, `DELETE FROM `+q.table+` WHERE expires_at < now()`)
		if err != nil {
			return p, err
		}
		*q.out = tag.RowsAffected()
	}
	return p, nil
}
