//go:build onnx

// onnx needs cgo and libonnxruntime; the docker image builds with -tags onnx.
// without the tag onnx_disabled.go is compiled instead so stub dev works anywhere.
package encode

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"sync"

	"github.com/sugarme/tokenizer"
	"github.com/sugarme/tokenizer/pretrained"
	ort "github.com/yalue/onnxruntime_go"
)

// layout under MODEL_DIR: embed/ and rerank/, each with tokenizer.json and the graph as
// model.onnx (optimum export) or onnx/model.onnx (huggingface hub layout, eg Xenova/*)
const (
	onnxEmbedDir  = "embed"
	onnxRerankDir = "rerank"
	onnxMaxSeq    = 512
)

var onnxModelFiles = []string{"model.onnx", filepath.Join("onnx", "model.onnx")}

func findModelFile(dir string) (string, error) {
	for _, name := range onnxModelFiles {
		p := filepath.Join(dir, name)
		if _, err := os.Stat(p); err == nil {
			return p, nil
		}
	}
	return "", fmt.Errorf("no model.onnx or onnx/model.onnx in %s", dir)
}

var ortInit struct {
	once sync.Once
	err  error
}

// the runtime environment is process wide and may only be created once
func initRuntime(lib string) error {
	ortInit.once.Do(func() {
		if lib != "" {
			ort.SetSharedLibraryPath(lib)
		}
		ortInit.err = ort.InitializeEnvironment()
	})
	return ortInit.err
}

// one tokenizer + session pair; inputs are the graph names in graph order
type onnxModel struct {
	name    string
	tk      *tokenizer.Tokenizer
	session *ort.DynamicAdvancedSession
	inputs  []string
	outDims ort.Shape
	padID   int64
	mu      sync.Mutex // ort sessions are thread safe, but parallel runs only fight over the same cores
}

type onnx struct {
	info    Info
	pooling string
	embed   *onnxModel
	rerank  *onnxModel
}

func openONNX(cfg Config) (Encoder, error) {
	if err := checkModelDir(cfg.ModelDir); err != nil {
		return nil, err
	}
	if cfg.Pooling != "cls" && cfg.Pooling != "mean" {
		return nil, fmt.Errorf("onnx backend: pooling must be cls or mean, got %q", cfg.Pooling)
	}
	if err := initRuntime(cfg.OnnxLib); err != nil {
		return nil, fmt.Errorf("onnx backend: runtime: %w", err)
	}

	opts, err := ort.NewSessionOptions()
	if err != nil {
		return nil, err
	}
	defer opts.Destroy()
	_ = opts.SetIntraOpNumThreads(runtime.NumCPU())
	_ = opts.SetGraphOptimizationLevel(ort.GraphOptimizationLevelEnableAll)

	embed, err := openModel(filepath.Join(cfg.ModelDir, onnxEmbedDir), opts)
	if err != nil {
		return nil, fmt.Errorf("onnx backend: embed: %w", err)
	}
	rerank, err := openModel(filepath.Join(cfg.ModelDir, onnxRerankDir), opts)
	if err != nil {
		embed.close()
		return nil, fmt.Errorf("onnx backend: rerank: %w", err)
	}

	e := &onnx{pooling: cfg.Pooling, embed: embed, rerank: rerank}
	dim := 0
	if n := len(embed.outDims); n > 0 && embed.outDims[n-1] > 0 {
		dim = int(embed.outDims[n-1])
	}
	e.info = Info{Backend: "onnx", EmbedModel: embed.name, EmbedDim: dim, RerankModel: rerank.name}

	// warm up: the first run pays for lazy allocation and catches a broken export at startup, not on a user
	res, err := e.Embed(context.Background(), []string{"warm up"})
	if err != nil {
		e.Close()
		return nil, fmt.Errorf("onnx backend: warm up: %w", err)
	}
	if e.info.EmbedDim == 0 {
		e.info.EmbedDim = len(res.Vectors[0])
	}
	if _, err := e.Rerank(context.Background(), "warm up", []string{"warm up"}); err != nil {
		e.Close()
		return nil, fmt.Errorf("onnx backend: warm up: %w", err)
	}
	return e, nil
}

func checkModelDir(modelDir string) error {
	for _, sub := range []string{onnxEmbedDir, onnxRerankDir} {
		dir := filepath.Join(modelDir, sub)
		if _, err := findModelFile(dir); err != nil {
			return fmt.Errorf("onnx backend: %w", err)
		}
		if _, err := os.Stat(filepath.Join(dir, "tokenizer.json")); err != nil {
			return fmt.Errorf("onnx backend: missing tokenizer.json in %s", dir)
		}
	}
	return nil
}

func openModel(dir string, opts *ort.SessionOptions) (*onnxModel, error) {
	modelPath, err := findModelFile(dir)
	if err != nil {
		return nil, err
	}
	inputs, outputs, err := ort.GetInputOutputInfo(modelPath)
	if err != nil {
		return nil, err
	}
	if len(outputs) == 0 {
		return nil, errors.New("model has no outputs")
	}
	m := &onnxModel{name: modelName(dir), outDims: outputs[0].Dimensions}
	for _, in := range inputs {
		switch in.Name {
		case "input_ids", "attention_mask", "token_type_ids":
			m.inputs = append(m.inputs, in.Name)
		default:
			return nil, fmt.Errorf("unexpected model input %q", in.Name)
		}
	}

	m.tk, err = pretrained.FromFile(filepath.Join(dir, "tokenizer.json"))
	if err != nil {
		return nil, fmt.Errorf("tokenizer: %w", err)
	}
	m.tk.WithTruncation(&tokenizer.TruncationParams{MaxLength: onnxMaxSeq, Strategy: tokenizer.LongestFirst})
	for _, tok := range []string{"[PAD]", "<pad>"} { // bert vs xlm-r; padded positions are masked anyway
		if id, ok := m.tk.TokenToId(tok); ok {
			m.padID = int64(id)
			break
		}
	}

	m.session, err = ort.NewDynamicAdvancedSession(modelPath, m.inputs, []string{outputs[0].Name}, opts)
	if err != nil {
		return nil, err
	}
	return m, nil
}

// modelName reads optimum's config.json if present, else the directory name
func modelName(dir string) string {
	if raw, err := os.ReadFile(filepath.Join(dir, "config.json")); err == nil {
		var cfg struct {
			Name string `json:"_name_or_path"`
		}
		if json.Unmarshal(raw, &cfg) == nil && cfg.Name != "" {
			return cfg.Name
		}
	}
	return "onnx-" + filepath.Base(dir)
}

func (m *onnxModel) close() {
	if m.session != nil {
		_ = m.session.Destroy()
	}
}

// --- batching ---

type batch struct {
	ids, mask, types []int64 // flat, row major, padded to seq
	rows, seq        int
	tokens           int // sum of attention_mask
}

func buildBatch(encs []*tokenizer.Encoding, padID int64) batch {
	seq := 0
	for _, e := range encs {
		seq = max(seq, len(e.Ids))
	}
	b := batch{rows: len(encs), seq: seq}
	n := b.rows * seq
	b.ids, b.mask, b.types = make([]int64, n), make([]int64, n), make([]int64, n)
	for r, e := range encs {
		off := r * seq
		for i := range e.Ids {
			b.ids[off+i] = int64(e.Ids[i])
			b.mask[off+i] = int64(e.AttentionMask[i])
			b.types[off+i] = int64(e.TypeIds[i])
			b.tokens += e.AttentionMask[i]
		}
		for i := len(e.Ids); i < seq; i++ {
			b.ids[off+i] = padID
		}
	}
	return b
}

// run tokenises nothing itself; it takes encodings, runs the graph and returns the
// output tensor, which the caller must Destroy
func (m *onnxModel) run(ctx context.Context, encs []*tokenizer.Encoding) (*ort.Tensor[float32], batch, error) {
	b := buildBatch(encs, m.padID)
	if err := ctx.Err(); err != nil {
		return nil, b, err
	}

	shape := ort.NewShape(int64(b.rows), int64(b.seq))
	values := make([]ort.Value, 0, len(m.inputs))
	defer func() {
		for _, v := range values {
			_ = v.Destroy()
		}
	}()
	for _, name := range m.inputs {
		var data []int64
		switch name {
		case "input_ids":
			data = b.ids
		case "attention_mask":
			data = b.mask
		case "token_type_ids":
			data = b.types
		}
		t, err := ort.NewTensor(shape, data)
		if err != nil {
			return nil, b, err
		}
		values = append(values, t)
	}

	outputs := []ort.Value{nil} // nil: ort allocates, we own it
	m.mu.Lock()
	err := m.session.Run(values, outputs)
	m.mu.Unlock()
	if err != nil {
		return nil, b, err
	}
	out, ok := outputs[0].(*ort.Tensor[float32])
	if !ok {
		_ = outputs[0].Destroy()
		return nil, b, fmt.Errorf("unexpected output type %T", outputs[0])
	}
	return out, b, nil
}

// --- Encoder ---

func (e *onnx) Embed(ctx context.Context, texts []string) (EmbedResult, error) {
	encs := make([]*tokenizer.Encoding, len(texts))
	for i, t := range texts {
		enc, err := e.embed.tk.EncodeSingle(t, true)
		if err != nil {
			return EmbedResult{}, err
		}
		encs[i] = enc
	}
	out, b, err := e.embed.run(ctx, encs)
	if err != nil {
		return EmbedResult{}, err
	}
	defer out.Destroy()

	shape := out.GetShape() // [rows, seq, dim]
	if len(shape) != 3 {
		return EmbedResult{}, fmt.Errorf("embed output has shape %v, want [batch, seq, dim]", shape)
	}
	dim := int(shape[2])
	data := out.GetData()

	res := EmbedResult{Vectors: make([][]float32, b.rows), Usage: Usage{InputTokens: b.tokens}}
	for r := 0; r < b.rows; r++ {
		v := make([]float32, dim)
		row := data[r*b.seq*dim : (r+1)*b.seq*dim]
		if e.pooling == "cls" {
			copy(v, row[:dim]) // token 0
		} else {
			meanPool(v, row, b.mask[r*b.seq:(r+1)*b.seq], dim)
		}
		l2normalize(v)
		res.Vectors[r] = v
	}
	return res, nil
}

func (e *onnx) Rerank(ctx context.Context, query string, docs []string) (RerankResult, error) {
	encs := make([]*tokenizer.Encoding, len(docs))
	for i, d := range docs {
		enc, err := e.rerank.tk.EncodePair(query, d, true)
		if err != nil {
			return RerankResult{}, err
		}
		encs[i] = enc
	}
	out, b, err := e.rerank.run(ctx, encs)
	if err != nil {
		return RerankResult{}, err
	}
	defer out.Destroy()

	data := out.GetData() // [rows, 1] or [rows]
	if len(data) < b.rows {
		return RerankResult{}, fmt.Errorf("rerank output has %d values for %d docs", len(data), b.rows)
	}
	res := RerankResult{Scores: make([]float32, b.rows), Usage: Usage{InputTokens: b.tokens}}
	stride := len(data) / b.rows
	for i := range res.Scores {
		res.Scores[i] = data[i*stride] // raw logit; order is what matters, sigmoid is the clients call
	}
	return res, nil
}

func (e *onnx) Info() Info { return e.info }

func (e *onnx) Close() error {
	e.embed.close()
	e.rerank.close()
	return ort.DestroyEnvironment()
}

// --- maths ---

// meanPool averages the token vectors where mask == 1; accumulates in float64
func meanPool(dst []float32, row []float32, mask []int64, dim int) {
	acc := make([]float64, dim)
	var n float64
	for t, m := range mask {
		if m == 0 {
			continue
		}
		n++
		tok := row[t*dim : (t+1)*dim]
		for i, x := range tok {
			acc[i] += float64(x)
		}
	}
	if n == 0 {
		return
	}
	for i := range dst {
		dst[i] = float32(acc[i] / n)
	}
}

func l2normalize(v []float32) {
	var s float64
	for _, x := range v {
		s += float64(x) * float64(x)
	}
	if s == 0 {
		return
	}
	f := float32(1 / math.Sqrt(s))
	for i := range v {
		v[i] *= f
	}
}
