//go:build !onnx

package encode

import "fmt"

// the real backend lives in onnx.go and needs cgo + libonnxruntime; build with -tags onnx (the dockerfile does)
func openONNX(Config) (Encoder, error) {
	return nil, fmt.Errorf("%w: binary built without -tags onnx", ErrNotImplemented)
}
