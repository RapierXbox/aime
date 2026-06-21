#!/bin/bash
cargo build "${@:2}" || exit $?
codesign -fs "aime-dev" --identifier "com.aime-org.aime" target/debug/app
echo "codesign res: $?"

# From: Claude Fable 5
# Re-authorize the new build for the "aime" keychain item.
#
# Self-signed certs have no Team ID, so the macOS login-keychain partition list
# falls back to the binary's cdhash — which changes every rebuild. Without this,
# every run re-prompts for the keychain password even after "Always Allow".
# We add the fresh cdhash to the item's partition list so reads pass silently.
#
# KEYCHAIN_PW = your macOS login password, set in src-tauri/.env (gitignored).
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
[ -f "$SCRIPT_DIR/.env" ] && set -a && source "$SCRIPT_DIR/.env" && set +a
if [ -n "$KEYCHAIN_PW" ]; then
  CDHASH=$(codesign -d --verbose=4 target/debug/app 2>&1 | grep '^CDHash=' | cut -d= -f2)
  # Item won't exist on the very first run; ignore the error — the app creates it
  # (auto-trusting the current cdhash), and later rebuilds get re-authorized here.
  security set-generic-password-partition-list \
    -S "cdhash:$CDHASH" -k "$KEYCHAIN_PW" \
    -s aime -a testsss "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1 \
    && echo "partition list updated for cdhash:$CDHASH" \
    || echo "partition list update skipped (item not created yet?)"
else
  echo "KEYCHAIN_PW not set — keychain may prompt on rebuild (see src-tauri/.env)"
fi

exec target/debug/app
