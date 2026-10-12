package encode

import (
	"context"
	"crypto/sha256"
	"encoding/binary"
	"math"
	"math/rand/v2"
	"strings"
)

// stub: deterministic fake backend, no model files. hash based vectors, word overlap scores.
type stub struct {
	dim int
}

// bge-small dim
func newStub() *stub { return &stub{dim: 384} }

func (s *stub) Embed(ctx context.Context, texts []string) (EmbedResult, error) {
	res := EmbedResult{Vectors: make([][]float32, len(texts))}
	for i, t := range texts {
		if err := ctx.Err(); err != nil {
			return EmbedResult{}, err
		}
		res.Vectors[i] = s.vector(t)
		res.Usage.InputTokens += approxTokens(t)
	}
	return res, nil
}

// unit vector seeded by the text hash
func (s *stub) vector(text string) []float32 {
	sum := sha256.Sum256([]byte(text))
	rng := rand.New(rand.NewPCG(binary.LittleEndian.Uint64(sum[:8]), binary.LittleEndian.Uint64(sum[8:16])))

	v := make([]float32, s.dim)
	var norm float64
	for i := range v {
		v[i] = float32(rng.NormFloat64())
		norm += float64(v[i]) * float64(v[i])
	}
	scale := float32(1 / math.Sqrt(norm))
	for i := range v {
		v[i] *= scale
	}
	return v
}

func (s *stub) Rerank(ctx context.Context, query string, docs []string) (RerankResult, error) {
	q := wordSet(query)
	res := RerankResult{Scores: make([]float32, len(docs))}
	for i, d := range docs {
		if err := ctx.Err(); err != nil {
			return RerankResult{}, err
		}
		res.Scores[i] = overlap(q, wordSet(d))
		// cross encoder sees query+doc per pair
		res.Usage.InputTokens += approxTokens(query) + approxTokens(d)
	}
	return res, nil
}

func (s *stub) Info() Info {
	return Info{Backend: "stub", EmbedModel: "stub-hash-v1", EmbedDim: s.dim, RerankModel: "stub-overlap-v1"}
}

func (s *stub) Close() error { return nil }

// ~words * 1.3
func approxTokens(text string) int {
	n := len(strings.Fields(text))
	return n + (n+2)/3
}

func wordSet(text string) map[string]struct{} {
	set := make(map[string]struct{})
	for _, w := range strings.Fields(strings.ToLower(text)) {
		if w = strings.Trim(w, `.,;:!?"'()[]`); w != "" {
			set[w] = struct{}{}
		}
	}
	return set
}

// fraction of query words found in doc
func overlap(query, doc map[string]struct{}) float32 {
	if len(query) == 0 || len(doc) == 0 {
		return 0
	}
	hits := 0
	for w := range query {
		if _, ok := doc[w]; ok {
			hits++
		}
	}
	return float32(hits) / float32(len(query))
}
