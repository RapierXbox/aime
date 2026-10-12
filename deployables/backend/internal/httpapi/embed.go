package httpapi

import (
	"context"
	"net/http"

	"github.com/rapierxbox/aime/backend/internal/encode"
)

type embedReq struct {
	Texts []string `json:"texts"`
}

type embedRes struct {
	Model   string      `json:"model"`
	Dim     int         `json:"dim"`
	Vectors [][]float32 `json:"vectors"` // same order as texts
	Usage   usageRes    `json:"usage"`
}

func (s *Server) handleEmbed(w http.ResponseWriter, r *http.Request) {
	accountID, ok := AccountFromContext(r.Context())
	if !ok {
		s.writeUnauthorized(w)
		return
	}

	var req embedReq
	if err := s.decodeJSONLimit(w, r, &req, maxInferBody); err != nil {
		return
	}
	if err := encode.ValidateTexts(req.Texts); err != nil {
		s.writeError(w, http.StatusBadRequest, err.Error())
		return
	}

	var res encode.EmbedResult
	receipt, err := s.runInference(r.Context(), accountID, encode.KindEmbed, len(req.Texts), func(ctx context.Context) (encode.Usage, error) {
		var err error
		res, err = s.Encoder.Embed(ctx, req.Texts)
		return res.Usage, err
	})
	if err != nil {
		s.writeInferError(w, err)
		return
	}

	info := s.Encoder.Info()
	s.writeJSON(w, http.StatusOK, embedRes{
		Model:   info.EmbedModel,
		Dim:     info.EmbedDim,
		Vectors: res.Vectors,
		Usage:   newUsageRes(res.Usage, receipt),
	})
}
