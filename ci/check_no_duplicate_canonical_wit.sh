#!/usr/bin/env bash
set -euo pipefail

PATTERN='^package\s+greentic:component@'

if command -v rg >/dev/null 2>&1; then
  MATCHES="$(rg -n --hidden --glob '!.git/*' --glob '*.wit' \
    --glob '!**/target/**' \
    --glob '!**/out/**' \
    --glob '!crates/vendor/**' \
    --glob '!**/wit-staging/**' \
    "$PATTERN" . || true)"
else
  MATCHES="$(find . \
    -path './.git' -prune -o \
    -path '*/target/*' -prune -o \
    -path '*/out/*' -prune -o \
    -path './crates/vendor/*' -prune -o \
    -path '*/wit-staging/*' -prune -o \
    -name '*.wit' -exec grep -nH -E "$PATTERN" {} + 2>/dev/null || true)"
fi

if [[ -n "$MATCHES" ]]; then
  echo "ERROR: greentic-pack must not define canonical greentic:component WIT."
  echo
  echo "$MATCHES"
  exit 1
fi

echo "OK: No canonical greentic:component WIT found."
