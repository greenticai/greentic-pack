#!/usr/bin/env bash
set -euo pipefail

PATTERN='greentic_interfaces::bindings::|\bbindings::greentic::'

MATCHES="$(rg -n --hidden \
  --glob '!.git/*' \
  --glob '!**/target/**' \
  --glob '!crates/vendor/**' \
  --glob '!**/*.lock' \
  "$PATTERN" crates README.md docs examples 2>/dev/null || true)"

if [[ -n "$MATCHES" ]]; then
  echo "ERROR: use greentic_interfaces::canonical instead of bindings::* in downstream code."
  echo
  echo "$MATCHES"
  exit 1
fi

echo "OK: no greentic_interfaces::bindings imports found in downstream code paths."
