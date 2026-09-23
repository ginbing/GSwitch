#!/usr/bin/env bash
set -euo pipefail
directory="$1"
test -n "${TAURI_UPDATER_PUBLIC_KEY:-}"
scratch="$(mktemp -d)"
trap 'rm -rf -- "$scratch"' EXIT
printf '%s' "$TAURI_UPDATER_PUBLIC_KEY" | base64 --decode > "$scratch/public.key"
grep -q '^untrusted comment: minisign public key:' "$scratch/public.key"
count=0
for encoded in "$directory"/*.sig; do
  test -f "$encoded"
  printf '%s' "$(cat "$encoded")" | base64 --decode > "$scratch/signature"
  minisign -Vm "${encoded%.sig}" -p "$scratch/public.key" -x "$scratch/signature"
  count=$((count + 1))
done
test "$count" -eq 5
echo "Verified $count final updater signatures"
