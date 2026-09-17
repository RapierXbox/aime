// smoke drives the whole api once against a running server and fails on the first surprise.
//
//	go run ./cmd/smoke -url http://localhost:8080
//
// needs BILLING_ENFORCE=false on the server (fresh accounts have no credits)
package main

import (
	"bytes"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/cloudflare/circl/sign/mldsa/mldsa65"
)

var (
	baseURL = flag.String("url", "http://localhost:8080", "server base url")
	client  = &http.Client{Timeout: 60 * time.Second}
	token   string
	step    int
)

func main() {
	flag.Parse()
	log.SetFlags(0)

	email := fmt.Sprintf("smoke-%s@example.com", hex.EncodeToString(randBytes(4)))
	password := "correct horse battery staple"

	// --- health ---
	expect("GET", "/healthz", nil, nil, 200)
	expect("GET", "/readyz", nil, nil, 200)
	metrics := string(body(expect("GET", "/metrics", nil, nil, 200)))
	must(strings.Contains(metrics, "aime_http_request_duration_seconds"), "metrics missing aime_ series")

	// --- account + device auth ---
	expect("GET", "/v1/account", nil, nil, 401)
	acct := decode(expect("POST", "/v1/accounts", nil, obj{"email": email, "password": password}, 201))
	expect("POST", "/v1/accounts", nil, obj{"email": email, "password": password}, 409)
	expect("POST", "/v1/accounts", nil, obj{"email": email, "password": "short"}, 400)

	enroll := decode(expect("POST", "/v1/auth/password", nil, obj{"email": email, "password": password}, 200))
	expect("POST", "/v1/auth/password", nil, obj{"email": email, "password": "wrong password!!"}, 401)

	pk, sk, err := mldsa65.GenerateKey(rand.Reader)
	must(err == nil, "keygen: %v", err)
	pkBytes, _ := pk.MarshalBinary()
	dev := decode(expect("POST", "/v1/devices", nil, obj{"enrollment_token": enroll["enrollment_token"], "name": "smoke laptop", "public_key": pkBytes}, 201))
	expect("POST", "/v1/devices", nil, obj{"enrollment_token": enroll["enrollment_token"], "name": "again", "public_key": pkBytes}, 401) // single use
	deviceID := int64(dev["device_id"].(float64))

	token = login(deviceID, sk)
	expect("POST", "/v1/auth/challange", nil, obj{"device_id": 999999999}, 404)

	me := decode(expect("GET", "/v1/account", nil, nil, 200))
	must(me["email"] == email, "account email mismatch: %v", me)
	must(int64(acct["account_id"].(float64)) == int64(me["account_id"].(float64)), "account id mismatch")

	// --- inference (stub backend) ---
	emb := decode(expect("POST", "/v1/embed", nil, obj{"texts": []string{"hello world", "invoice due friday"}}, 200))
	vectors := emb["vectors"].([]any)
	must(len(vectors) == 2, "embed: want 2 vectors, got %d", len(vectors))
	must(len(vectors[0].([]any)) == int(emb["dim"].(float64)), "embed: dim mismatch")
	usage := emb["usage"].(map[string]any)
	must(usage["input_tokens"].(float64) > 0 && usage["cost_microcredits"].(float64) > 0, "embed: usage/cost missing: %v", usage)
	expect("POST", "/v1/embed", nil, obj{"texts": []string{}}, 400)
	expect("POST", "/v1/embed", nil, obj{"texts": []string{"x"}, "extra": 1}, 400)

	rr := decode(expect("POST", "/v1/rerank", nil, obj{"query": "invoice due", "docs": []string{"nice weather", "your invoice is due", "invoice"}, "top_k": 2}, 200))
	results := rr["results"].([]any)
	must(len(results) == 2, "rerank: top_k not applied")
	must(int(results[0].(map[string]any)["index"].(float64)) == 1, "rerank: best doc should be index 1: %v", results)

	// --- backups: full, delta, list, download, in-use, prune, delete ---
	blob1 := randBytes(1 << 20)
	sum1 := sha256hex(blob1)
	b1 := decode(expectRaw("POST", "/v1/backups", map[string]string{"X-Backup-Checksum": sum1, "X-Backup-Meta": `{"nonce":"abc","kdf":{"t":3}}`}, blob1, 201))
	must(b1["kind"] == "full" && b1["checksum"] == sum1 && b1["version"].(float64) == 1, "backup 1: %v", b1)
	id1 := int64(b1["id"].(float64))
	expectRaw("POST", "/v1/backups", map[string]string{"X-Backup-Checksum": strings.Repeat("0", 64)}, blob1, 400) // checksum mismatch
	expectRaw("POST", "/v1/backups", map[string]string{"X-Backup-Meta": "{\"x\":\"ä\"}"}, blob1, 400)             // non ascii meta
	expectRaw("POST", "/v1/backups", nil, nil, 400)                                                               // empty

	blob2 := randBytes(64 << 10)
	b2 := decode(expectRaw("POST", "/v1/backups", map[string]string{"X-Backup-Parent": fmt.Sprint(id1)}, blob2, 201))
	must(b2["kind"] == "delta" && b2["version"].(float64) == 2, "backup 2: %v", b2)
	id2 := int64(b2["id"].(float64))
	expectRaw("POST", "/v1/backups", map[string]string{"X-Backup-Parent": fmt.Sprint(id1)}, blob2, 409) // stale parent

	list := decode(expect("GET", "/v1/backups", nil, nil, 200))
	must(len(list["backups"].([]any)) == 2, "list: want 2, got %v", list)
	must(int64(list["storage_used_bytes"].(float64)) == int64(len(blob1)+len(blob2)), "list: storage_used_bytes wrong")

	res := expect("GET", fmt.Sprintf("/v1/backups/%d", id2), nil, nil, 200)
	got := body(res)
	must(bytes.Equal(got, blob2), "download: bytes differ")
	must(res.Header.Get("X-Backup-Parent") == fmt.Sprint(id1), "download: parent header missing")
	res1 := expect("GET", fmt.Sprintf("/v1/backups/%d", id1), nil, nil, 200)
	must(res1.Header.Get("X-Backup-Meta") == `{"nonce":"abc","kdf":{"t":3}}`, "download: meta not byte exact: %q", res1.Header.Get("X-Backup-Meta"))
	must(bytes.Equal(body(res1), blob1), "download: bytes differ") // and drain it, an open download pins the file on windows
	expect("GET", "/v1/backups/999999999", nil, nil, 404)

	expect("DELETE", fmt.Sprintf("/v1/backups/%d", id1), nil, nil, 409) // parent of a delta
	expect("DELETE", "/v1/backups?before_version=2", nil, nil, 409)     // boundary is a delta

	b3 := decode(expectRaw("POST", "/v1/backups", nil, randBytes(1024), 201))
	must(b3["version"].(float64) == 3, "backup 3: %v", b3)
	pr := decode(expect("DELETE", "/v1/backups?before_version=3", nil, nil, 200))
	must(pr["deleted"].(float64) == 2, "prune: %v", pr)
	expect("DELETE", fmt.Sprintf("/v1/backups/%d", int64(b3["id"].(float64))), nil, nil, 204)
	list = decode(expect("GET", "/v1/backups", nil, nil, 200))
	must(len(list["backups"].([]any)) == 0, "list after delete: %v", list)

	// version numbers keep climbing after deletes
	b4 := decode(expectRaw("POST", "/v1/backups", nil, randBytes(10), 201))
	must(b4["version"].(float64) == 4, "version must not be reused: %v", b4)
	expect("DELETE", fmt.Sprintf("/v1/backups/%d", int64(b4["id"].(float64))), nil, nil, 204)

	// --- usage, devices, enrollment, password ---
	us := decode(expect("GET", "/v1/account/usage?days=7", nil, nil, 200))
	must(us["total"].(map[string]any)["calls"].(float64) >= 2, "usage: %v", us)
	expect("GET", "/v1/account/usage?days=0", nil, nil, 400)

	devs := decode(expect("GET", "/v1/devices", nil, nil, 200))
	d0 := devs["devices"].([]any)[0].(map[string]any)
	must(d0["current"] == true && d0["name"] == "smoke laptop", "devices: %v", devs)
	expect("PATCH", fmt.Sprintf("/v1/devices/%d", deviceID), nil, obj{"name": "renamed"}, 204)
	expect("PATCH", "/v1/devices/999999999", nil, obj{"name": "nope"}, 404)

	expect("POST", "/v1/enrollment-tokens", nil, obj{"password": "wrong password!!"}, 401)
	expect("POST", "/v1/enrollment-tokens", nil, obj{"password": password}, 201)

	newPassword := "even more correct horse"
	expect("POST", "/v1/account/password", nil, obj{"current_password": "wrong password!!", "new_password": newPassword}, 401)
	expect("POST", "/v1/account/password", nil, obj{"current_password": password, "new_password": newPassword}, 204)
	expect("GET", "/v1/account", nil, nil, 200) // own session survives
	expect("POST", "/v1/auth/password", nil, obj{"email": email, "password": password}, 401)
	expect("POST", "/v1/auth/password", nil, obj{"email": email, "password": newPassword}, 200)

	// --- logout, re-login with the device key, delete account ---
	expect("POST", "/v1/auth/logout", nil, nil, 204)
	expect("GET", "/v1/account", nil, nil, 401)
	token = login(deviceID, sk)
	la := decode(expect("POST", "/v1/auth/logout-all", nil, nil, 200))
	must(la["revoked"].(float64) >= 1, "logout-all: %v", la)
	expect("GET", "/v1/account", nil, nil, 401)
	token = login(deviceID, sk)
	expect("DELETE", "/v1/account", nil, obj{"password": newPassword}, 204)
	expect("GET", "/v1/account", nil, nil, 401)
	expect("POST", "/v1/auth/challange", nil, obj{"device_id": deviceID}, 404) // cascaded

	log.Printf("\nsmoke ok: %d requests behaved", step)
}

// login runs challange -> sign -> verify for a device and returns the session token
func login(deviceID int64, sk *mldsa65.PrivateKey) string {
	token = ""
	ch := decode(expect("POST", "/v1/auth/challange", nil, obj{"device_id": deviceID}, 201))
	nonce := decodeB64(ch["nonce"].(string))
	sig := make([]byte, mldsa65.SignatureSize)
	must(mldsa65.SignTo(sk, nonce, []byte("aime-auth-v1"), true, sig) == nil, "sign failed")
	expect("POST", "/v1/auth/verify", nil, obj{"challange_id": ch["challange_id"], "nonce": nonce, "signature": sig[:10]}, 401) // bad sig
	// the challange is consumed by the failed attempt, get a fresh one
	ch = decode(expect("POST", "/v1/auth/challange", nil, obj{"device_id": deviceID}, 201))
	nonce = decodeB64(ch["nonce"].(string))
	must(mldsa65.SignTo(sk, nonce, []byte("aime-auth-v1"), true, sig) == nil, "sign failed")
	expect("POST", "/v1/auth/verify", nil, obj{"challange_id": ch["challange_id"], "nonce": randBytes(32), "signature": sig}, 400) // wrong nonce
	ch = decode(expect("POST", "/v1/auth/challange", nil, obj{"device_id": deviceID}, 201))
	nonce = decodeB64(ch["nonce"].(string))
	must(mldsa65.SignTo(sk, nonce, []byte("aime-auth-v1"), true, sig) == nil, "sign failed")
	v := decode(expect("POST", "/v1/auth/verify", nil, obj{"challange_id": ch["challange_id"], "nonce": nonce, "signature": sig}, 200))
	return v["token"].(string)
}

// --- helpers ---

type obj map[string]any

func expect(method, path string, headers map[string]string, jsonBody any, want int) *http.Response {
	var payload []byte
	if jsonBody != nil {
		payload, _ = json.Marshal(jsonBody)
		if headers == nil {
			headers = map[string]string{}
		}
		headers["Content-Type"] = "application/json"
	}
	return expectRaw(method, path, headers, payload, want)
}

func expectRaw(method, path string, headers map[string]string, payload []byte, want int) *http.Response {
	step++
	req, err := http.NewRequest(method, *baseURL+path, bytes.NewReader(payload))
	must(err == nil, "request: %v", err)
	req.ContentLength = int64(len(payload))
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	res, err := client.Do(req)
	must(err == nil, "%s %s: %v", method, path, err)
	// the strict limiter allows 10 argon2 calls per burst from one ip; a real client waits too
	for attempt := 0; res.StatusCode == 429 && want != 429 && attempt < 5; attempt++ {
		res.Body.Close()
		log.Printf("    %-6s %-40s 429, waiting for the rate limiter", method, path)
		time.Sleep(3500 * time.Millisecond)
		req.Body = io.NopCloser(bytes.NewReader(payload))
		res, err = client.Do(req)
		must(err == nil, "%s %s: %v", method, path, err)
	}
	if res.StatusCode != want {
		b, _ := io.ReadAll(res.Body)
		log.Fatalf("step %d: %s %s: got %d want %d: %s", step, method, path, res.StatusCode, want, strings.TrimSpace(string(b)))
	}
	log.Printf("ok  %-6s %-40s %d", method, path, res.StatusCode)
	return res
}

func body(res *http.Response) []byte {
	defer res.Body.Close()
	b, err := io.ReadAll(res.Body)
	must(err == nil, "read body: %v", err)
	return b
}

func decode(res *http.Response) map[string]any {
	var m map[string]any
	must(json.Unmarshal(body(res), &m) == nil, "response is not a json object")
	return m
}

func decodeB64(s string) []byte {
	var out []byte
	must(json.Unmarshal([]byte(`"`+s+`"`), &out) == nil, "bad base64 nonce")
	return out
}

func randBytes(n int) []byte {
	b := make([]byte, n)
	_, _ = rand.Read(b)
	return b
}

func sha256hex(b []byte) string {
	s := sha256.Sum256(b)
	return hex.EncodeToString(s[:])
}

func must(ok bool, format string, args ...any) {
	if !ok {
		log.Printf("step %d: "+format, append([]any{step}, args...)...)
		os.Exit(1)
	}
}
