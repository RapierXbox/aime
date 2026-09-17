// Package blobs stores opaque files under one root. paths are generated here,
// never taken from clients; the db only keeps the relative path.
package blobs

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

var ErrBadPath = errors.New("path escapes blob root")

type Dir struct {
	root string // absolute
}

func Open(root string) (*Dir, error) {
	abs, err := filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(abs, 0o700); err != nil {
		return nil, err
	}
	// a bind mount with the wrong owner would otherwise turn every upload into a 500
	probe := filepath.Join(abs, ".probe")
	if err := os.WriteFile(probe, nil, 0o600); err != nil {
		return nil, fmt.Errorf("blob root not writable: %w", err)
	}
	_ = os.Remove(probe)
	return &Dir{root: abs}, nil
}

// Write streams r into a new file under <owner>/ and returns the relative path, size and hex sha256.
// written to a .part file first, so a torn upload never shows up as a complete blob
func (d *Dir) Write(owner int64, r io.Reader) (rel string, size int64, sum string, err error) {
	ownerDir := strconv.FormatInt(owner, 10)
	if err = os.MkdirAll(filepath.Join(d.root, ownerDir), 0o700); err != nil {
		return "", 0, "", err
	}

	var rnd [16]byte
	if _, err = rand.Read(rnd[:]); err != nil {
		return "", 0, "", err
	}
	rel = ownerDir + "/" + hex.EncodeToString(rnd[:]) + ".bin"
	final := filepath.Join(d.root, filepath.FromSlash(rel))
	part := final + ".part"

	f, err := os.OpenFile(part, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return "", 0, "", err
	}
	h := sha256.New()
	size, err = io.Copy(io.MultiWriter(f, h), r)
	if err == nil {
		err = f.Sync()
	}
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err == nil {
		err = os.Rename(part, final)
	}
	if err != nil {
		_ = os.Remove(part)
		return "", 0, "", err
	}
	syncDir(filepath.Dir(final)) // the rename itself is durable only once the directory is
	return rel, size, hex.EncodeToString(h.Sum(nil)), nil
}

func syncDir(p string) {
	if d, err := os.Open(p); err == nil {
		_ = d.Sync() // not supported on every fs, best effort
		_ = d.Close()
	}
}

func (d *Dir) Open(rel string) (*os.File, error) {
	p, err := d.resolve(rel)
	if err != nil {
		return nil, err
	}
	return os.Open(p)
}

// Remove deletes a blob; a missing file is not an error
func (d *Dir) Remove(rel string) error {
	p, err := d.resolve(rel)
	if err != nil {
		return err
	}
	if err := os.Remove(p); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return err
	}
	return nil
}

// resolve refuses anything that is not a plain file path inside root, even though rel only ever comes from our own db
func (d *Dir) resolve(rel string) (string, error) {
	if rel == "" || strings.Contains(rel, "..") || filepath.IsAbs(rel) {
		return "", ErrBadPath
	}
	p := filepath.Join(d.root, filepath.FromSlash(rel))
	if !strings.HasPrefix(p, d.root+string(filepath.Separator)) {
		return "", ErrBadPath
	}
	return p, nil
}

type SweepResult struct {
	Files int
	Bytes int64
}

// Sweep removes files nobody references and empty owner dirs. files younger than
// minAge are left alone: an upload writes its file before the row exists.
// symlinks are never followed
func (d *Dir) Sweep(keep func(rel string) bool, minAge time.Duration) (SweepResult, error) {
	var res SweepResult
	cutoff := time.Now().Add(-minAge)
	err := filepath.WalkDir(d.root, func(p string, e fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if p == d.root || e.IsDir() {
			return nil
		}
		info, err := e.Info()
		if err != nil || !info.Mode().IsRegular() || info.ModTime().After(cutoff) {
			return nil
		}
		rel, err := filepath.Rel(d.root, p)
		if err != nil {
			return nil
		}
		if keep(filepath.ToSlash(rel)) {
			return nil
		}
		if err := os.Remove(p); err == nil {
			res.Files++
			res.Bytes += info.Size()
		}
		return nil
	})
	if err != nil {
		return res, err
	}

	// owner dirs left empty after deletes
	entries, err := os.ReadDir(d.root)
	if err != nil {
		return res, err
	}
	for _, e := range entries {
		if e.IsDir() {
			_ = os.Remove(filepath.Join(d.root, e.Name())) // fails while non empty, which is fine
		}
	}
	return res, nil
}
