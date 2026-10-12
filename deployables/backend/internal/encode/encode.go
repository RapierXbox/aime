package encode

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"unicode/utf8"
)

type Kind string

const (
	KindEmbed  Kind = "embed"
	KindRerank Kind = "rerank"
)

// per request limits
const (
	MaxTexts     = 64   // texts per embed call / docs per rerank call
	MaxTextChars = 8192 // runes per text, ~2k tokens
)

var (
	ErrNoInput        = errors.New("no input")
	ErrEmptyInput     = errors.New("empty input")
	ErrTooManyInputs  = errors.New("too many inputs")
	ErrInputTooLong   = errors.New("input too long")
	ErrUnknownBackend = errors.New("unknown backend")
	ErrNotImplemented = errors.New("backend not implemented")
)

type Usage struct {
	InputTokens int
}

type EmbedResult struct {
	Vectors [][]float32 // one per input text, same order, l2 normalized
	Usage   Usage
}

type RerankResult struct {
	Scores []float32 // one per doc, same order as the input, higher = more relevant
	Usage  Usage
}

type Info struct {
	Backend     string
	EmbedModel  string
	EmbedDim    int
	RerankModel string
}

type Encoder interface {
	Embed(ctx context.Context, texts []string) (EmbedResult, error)
	Rerank(ctx context.Context, query string, docs []string) (RerankResult, error)
	Info() Info
	Close() error
}

type Config struct {
	Backend  string // stub | onnx | hailo
	ModelDir string
	Pooling  string // cls | mean, what the embed model was trained with
	OnnxLib  string // path to libonnxruntime, empty = default lookup
}

func Open(cfg Config) (Encoder, error) {
	switch cfg.Backend {
	case "stub":
		return newStub(), nil
	case "onnx":
		return openONNX(cfg)
	case "hailo":
		return nil, fmt.Errorf("%w: hailo", ErrNotImplemented)
	default:
		return nil, fmt.Errorf("%w: %q", ErrUnknownBackend, cfg.Backend)
	}
}

// errors are safe to show to clients
func ValidateText(text string) error {
	if strings.TrimSpace(text) == "" {
		return ErrEmptyInput
	}
	if utf8.RuneCountInString(text) > MaxTextChars {
		return fmt.Errorf("%w: max %d chars", ErrInputTooLong, MaxTextChars)
	}
	return nil
}

func ValidateTexts(texts []string) error {
	if len(texts) == 0 {
		return ErrNoInput
	}
	if len(texts) > MaxTexts {
		return fmt.Errorf("%w: %d, max %d", ErrTooManyInputs, len(texts), MaxTexts)
	}
	for i, t := range texts {
		if err := ValidateText(t); err != nil {
			return fmt.Errorf("%w (index %d)", err, i)
		}
	}
	return nil
}
