#!/bin/sh
set -eu
cd "$(dirname "$0")/../src-tauri"
fixture_url="$(python3 ../scripts/prepare-test-db.py)"
trap 'rm -f "${fixture_url#sqlite:}"' EXIT
DATABASE_URL="$fixture_url" cargo test --lib "$@"
