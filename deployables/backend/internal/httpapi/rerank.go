package httpapi

import (
	"context"
	"net/http"
	"sort"

	"github.com/rapierxbox/aime/backend/internal/encode"
)

type rerankReq struct {
	Query string   `json:"query"`
	Docs  []string `json:"docs"`
	TopK  int      `json:"top_k"` // optional, 0 = return every doc
}

type rerankHit struct {
	Index int     `json:"index"` // position in the request docs
	Score float32 `json:"score"`
}

type rerankRes struct {
	Model   string      `json:"model"`
	Results []rerankHit `json:"results"` // best first
	Usage   usageRes    `json:"usage"`
}

func (s *Server) handleRerank(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}

	var req rerankReq
	if err := s.decodeJSONLimit(w, r, &req, maxInferBody); err != nil {
		return
	}
	if err := encode.ValidateText(req.Query); err != nil {
		s.writeError(w, http.StatusBadRequest, "query: "+err.Error())
		return
	}
	if err := encode.ValidateTexts(req.Docs); err != nil {
		s.writeError(w, http.StatusBadRequest, "docs: "+err.Error())
		return
	}
	if req.TopK < 0 {
		s.writeError(w, http.StatusBadRequest, "top_k must be >= 0")
		return
	}

	var res encode.RerankResult
	receipt, err := s.runInference(r.Context(), accountID, encode.KindRerank, len(req.Docs), func(ctx context.Context) (encode.Usage, error) {
		var err error
		res, err = s.Encoder.Rerank(ctx, req.Query, req.Docs)
		return res.Usage, err
	})
	if err != nil {
		s.writeInferError(w, err)
		return
	}

	hits := make([]rerankHit, len(res.Scores))
	for i, score := range res.Scores {
		hits[i] = rerankHit{Index: i, Score: score}
	}
	sort.SliceStable(hits, func(a, b int) bool { return hits[a].Score > hits[b].Score })
	if req.TopK > 0 && req.TopK < len(hits) {
		hits = hits[:req.TopK]
	}

	s.writeJSON(w, http.StatusOK, rerankRes{
		Model:   s.Encoder.Info().RerankModel,
		Results: hits,
		Usage:   newUsageRes(res.Usage, receipt),
	})
}
