// demo shows what the encoder does: a cosine similarity table over a few texts and a
// reranked query. it creates a throwaway account + device and deletes them afterwards.
//
//	go run ./cmd/demo
//	go run ./cmd/demo -texts "cat,kitten,invoice" -query "unpaid bill" -docs "the weather,your invoice is overdue"
//
// needs BILLING_ENFORCE=false on the server (fresh accounts have no credits)
package main

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"math"
	"net/http"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/cloudflare/circl/sign/mldsa/mldsa65"
)

var (
	baseURL = flag.String("url", "http://localhost:8080", "server base url")
	texts   = flag.String("texts", "cat,kitten,dog,invoice,unpaid bill,sunny weather", "comma separated texts to embed")
	query   = flag.String("query", "when is my invoice due", "query to rerank against -docs")
	docs    = flag.String("docs", "the weather will be sunny tomorrow,your invoice is due on friday,invoice,how to train a kitten", "comma separated documents")
	client  = &http.Client{Timeout: 120 * time.Second}
	token   string
)

func main() {
	flag.Parse()
	log.SetFlags(0)

	email := fmt.Sprintf("demo-%s@example.com", hex.EncodeToString(randBytes(4)))
	password := "demo password that is long enough"

	// throwaway account + device, deleted at the end
	call("POST", "/v1/accounts", obj{"email": email, "password": password}, 201)
	enroll := call("POST", "/v1/auth/password", obj{"email": email, "password": password}, 200)
	pk, sk, err := mldsa65.GenerateKey(rand.Reader)
	if err != nil {
		log.Fatal(err)
	}
	pkBytes, _ := pk.MarshalBinary()
	dev := call("POST", "/v1/devices", obj{"enrollment_token": enroll["enrollment_token"], "name": "demo", "public_key": pkBytes}, 201)
	deviceID := int64(dev["device_id"].(float64))
	ch := call("POST", "/v1/auth/challange", obj{"device_id": deviceID}, 201)
	nonce := b64(ch["nonce"].(string))
	sig := make([]byte, mldsa65.SignatureSize)
	if err := mldsa65.SignTo(sk, nonce, []byte("aime-auth-v1"), true, sig); err != nil {
		log.Fatal(err)
	}
	token = call("POST", "/v1/auth/verify", obj{"challange_id": ch["challange_id"], "nonce": nonce, "signature": sig}, 200)["token"].(string)
	defer call("DELETE", "/v1/account", obj{"password": password}, 204)

	// --- embed ---
	items := split(*texts)
	start := time.Now()
	emb := call("POST", "/v1/embed", obj{"texts": items}, 200)
	took := time.Since(start)
	vecs := toVectors(emb["vectors"])
	usage := emb["usage"].(map[string]any)
	fmt.Printf("\nembed  model=%s dim=%v texts=%d tokens=%v cost=%v microcredits  (%s)\n\n",
		emb["model"], emb["dim"], len(items), usage["input_tokens"], usage["cost_microcredits"], took.Round(time.Millisecond))

	w := 0
	for _, t := range items {
		w = max(w, len(t))
	}
	fmt.Printf("%*s", w+2, "")
	for _, t := range items {
		fmt.Printf("%8s", short(t, 8))
	}
	fmt.Println()
	for i, a := range vecs {
		fmt.Printf("%-*s", w+2, items[i])
		for _, b := range vecs {
			fmt.Printf("%8.2f", dot(a, b))
		}
		fmt.Println()
	}
	fmt.Println("\n(cosine similarity; vectors are unit length so this is a plain dot product)")

	// --- rerank ---
	ds := split(*docs)
	start = time.Now()
	rr := call("POST", "/v1/rerank", obj{"query": *query, "docs": ds}, 200)
	took = time.Since(start)
	usage = rr["usage"].(map[string]any)
	fmt.Printf("\nrerank model=%s query=%q docs=%d tokens=%v cost=%v microcredits  (%s)\n\n",
		rr["model"], *query, len(ds), usage["input_tokens"], usage["cost_microcredits"], took.Round(time.Millisecond))
	results := rr["results"].([]any)
	sort.SliceStable(results, func(i, j int) bool {
		return results[i].(map[string]any)["score"].(float64) > results[j].(map[string]any)["score"].(float64)
	})
	for rank, r := range results {
		m := r.(map[string]any)
		score := m["score"].(float64)
		fmt.Printf("  %d. %7.3f  (p=%.2f)  %s\n", rank+1, score, 1/(1+math.Exp(-score)), ds[int(m["index"].(float64))])
	}
	fmt.Println("\n(raw cross-encoder logit; p = sigmoid, only the order is guaranteed to mean something)")
}

// --- helpers ---

type obj map[string]any

func call(method, path string, body any, want int) map[string]any {
	var payload []byte
	if body != nil {
		payload, _ = json.Marshal(body)
	}
	req, _ := http.NewRequest(method, *baseURL+path, bytes.NewReader(payload))
	req.ContentLength = int64(len(payload))
	req.Header.Set("Content-Type", "application/json")
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	res, err := client.Do(req)
	if err != nil {
		log.Fatalf("%s %s: %v", method, path, err)
	}
	defer res.Body.Close()
	raw, _ := io.ReadAll(res.Body)
	if res.StatusCode != want {
		log.Fatalf("%s %s: %d %s", method, path, res.StatusCode, strings.TrimSpace(string(raw)))
	}
	var out map[string]any
	if len(raw) > 0 {
		if err := json.Unmarshal(raw, &out); err != nil {
			log.Fatalf("%s %s: bad json: %v", method, path, err)
		}
	}
	return out
}

func toVectors(v any) [][]float64 {
	rows := v.([]any)
	out := make([][]float64, len(rows))
	for i, r := range rows {
		cols := r.([]any)
		out[i] = make([]float64, len(cols))
		for j, c := range cols {
			out[i][j] = c.(float64)
		}
	}
	return out
}

func dot(a, b []float64) float64 {
	var s float64
	for i := range a {
		s += a[i] * b[i]
	}
	return s
}

func split(s string) []string {
	var out []string
	for _, p := range strings.Split(s, ",") {
		if p = strings.TrimSpace(p); p != "" {
			out = append(out, p)
		}
	}
	return out
}

func short(s string, n int) string {
	if len(s) <= n-1 {
		return s
	}
	return s[:n-2] + "…"
}

func b64(s string) []byte {
	var out []byte
	if err := json.Unmarshal([]byte(`"`+s+`"`), &out); err != nil {
		log.Fatal("bad nonce")
	}
	return out
}

func randBytes(n int) []byte {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		os.Exit(1)
	}
	return b
}
