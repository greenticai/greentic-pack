#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   LOCAL_CHECK_ONLINE=1 LOCAL_CHECK_STRICT=1 LOCAL_CHECK_VERBOSE=1 ci/local_check.sh
# Defaults: offline, non-strict, quiet.

: "${LOCAL_CHECK_ONLINE:=1}"
: "${LOCAL_CHECK_STRICT:=0}"
: "${LOCAL_CHECK_VERBOSE:=0}"

if [[ "$LOCAL_CHECK_VERBOSE" == "1" ]]; then
  set -x
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

need() {
  command -v "$1" >/dev/null 2>&1
}

print_version() {
  local tool="$1"
  if need "$tool"; then
    "$tool" --version || true
  else
    echo "[skip] $tool --version (not installed)"
  fi
}

step() {
  echo ""
  echo "▶ $*"
}

require_tool() {
  local tool="$1"
  local context="$2"
  if need "$tool"; then
    return 0
  fi
  echo "[miss] $tool ($context)"
  if [[ "$LOCAL_CHECK_STRICT" == "1" ]]; then
    echo "[fail] Missing required tool $tool in strict mode"
    return 1
  fi
  return 99
}

run_or_skip() {
  local desc="$1"
  shift
  if "$@"; then
    return 0
  fi
  local status=$?
  if [[ $status -eq 99 ]]; then
    echo "[skip] $desc"
    return 0
  fi
  echo "[fail] $desc"
  exit $status
}

install_greentic_dev() {
  if need greentic-dev; then
    return 0
  fi
  require_tool cargo-binstall "install greentic-dev via cargo-binstall" || return $?
  if ! can_reach_cratesio; then
    echo "[skip] greentic-dev install (crates.io unreachable)"
    return 0
  fi
  cargo binstall greentic-dev -y
}

fmt_check() {
  require_tool cargo "cargo fmt" || return $?
  cargo fmt --all -- --check
}

clippy_check() {
  require_tool cargo "cargo clippy" || return $?
  set +e
  can_reach_cratesio
  local reachable=$?
  set -e
  if [[ $reachable -ne 0 ]]; then
    echo "[skip] cargo clippy (crates.io unreachable)"
    return 0
  fi
  cargo clippy --workspace --all-targets -- -D warnings
}

build_check() {
  require_tool cargo "cargo build" || return $?
  set +e
  can_reach_cratesio
  local reachable=$?
  set -e
  if [[ $reachable -ne 0 ]]; then
    echo "[skip] cargo build (crates.io unreachable)"
    return 0
  fi
  cargo build --workspace --all-features --locked
}

test_check() {
  require_tool cargo "cargo test" || return $?
  set +e
  can_reach_cratesio
  local reachable=$?
  set -e
  if [[ $reachable -ne 0 ]]; then
    echo "[skip] cargo test (crates.io unreachable)"
    return 0
  fi
  cargo test --workspace --all-features --locked -- --nocapture
}

builder_demo_check() (
  require_tool cargo "builder demo" || return $?
  require_tool jq "builder demo report validation" || return $?
  require_tool unzip "builder demo unzip" || return $?
  require_tool diff "builder demo diff" || return $?

  local tmpdir
  tmpdir=$(mktemp -d)
  trap "rm -rf '$tmpdir'" RETURN
  local out1="$tmpdir/demo1.gtpack"
  local out2="$tmpdir/demo2.gtpack"

  run_demo() {
    local out="$1"
    local log="$2"
    if cargo run -p greentic-pack --example build_demo -- --out "$out" >"$log" 2>&1; then
      return 0
    fi
    if grep -q "Couldn't resolve host name" "$log" || grep -q "failed to download" "$log"; then
      echo "[skip] builder demo (crates.io unreachable)"
      return 99
    fi
    cat "$log"
    return 1
  }

  run_demo "$out1" "$tmpdir/demo1.log"
  status=$?
  if [[ $status -eq 99 ]]; then return 0; fi
  if [[ $status -ne 0 ]]; then return $status; fi

  run_demo "$out2" "$tmpdir/demo2.log"
  status=$?
  if [[ $status -eq 99 ]]; then return 0; fi
  if [[ $status -ne 0 ]]; then return $status; fi

  local unpack1="$tmpdir/unpack1"
  local unpack2="$tmpdir/unpack2"
  mkdir "$unpack1" "$unpack2"
  unzip -q "$out1" -d "$unpack1"
  unzip -q "$out2" -d "$unpack2"

  rm -rf "$unpack1/signatures" "$unpack2/signatures"
  if ! diff -rq "$unpack1" "$unpack2"; then
    echo "build demo outputs differ (excluding signatures)"
    return 1
  fi

  local report
  report=$(cargo run -p packc --bin greentic-pack -- --json doctor "$out1")
  echo "$report" | jq -e 'has("sbom") and (all(.sbom[]; (.media_type | length > 0)))' >/dev/null
)

packc_gtpack_check() {
  require_tool cargo "packc build" || return $?
  require_tool jq "packc gtpack inspect" || return $?
  if ! can_reach_cratesio; then
    echo "[skip] packc gtpack (crates.io unreachable)"
    return 0
  fi

  if [[ "$LOCAL_CHECK_ONLINE" != "1" ]]; then
    echo "[skip] packc gtpack (offline mode)"
    return 0
  fi

  local pack_dir="examples/weather-demo"
  local tmpdir_rel=".packc-check"
  local tmpdir="$pack_dir/$tmpdir_rel"
  rm -rf "$tmpdir"
  mkdir -p "$tmpdir"
  trap "rm -rf '$tmpdir'" RETURN
  local out_wasm="$tmpdir_rel/pack.wasm"
  local out_manifest="$tmpdir_rel/manifest.cbor"
  local out_sbom="$tmpdir_rel/sbom.cdx.json"
  local out_gtpack="$tmpdir_rel/pack.gtpack"

  local build_log="$tmpdir/packc-build.log"
  if ! cargo run -p packc --bin packc -- build \
    --in "$pack_dir" \
    --out "$out_wasm" \
    --manifest "$out_manifest" \
    --sbom "$out_sbom" \
    --gtpack-out "$out_gtpack" \
    --log warn \
    >"$build_log" 2>&1; then
    if grep -q "Couldn't resolve host name" "$build_log"; then
      echo "[skip] packc gtpack (crates.io unreachable)"
      return 0
    fi
    cat "$build_log"
    return 1
  fi

  local report
  report=$(cargo run -p packc --bin greentic-pack -- --json doctor "$pack_dir/$out_gtpack")
  echo "$report" | jq -e 'has("sbom") and (all(.sbom[]; (.media_type | length > 0)))' >/dev/null
}

main() {
  echo "LOCAL_CHECK_ONLINE=$LOCAL_CHECK_ONLINE"
  echo "LOCAL_CHECK_STRICT=$LOCAL_CHECK_STRICT"
  echo "LOCAL_CHECK_VERBOSE=$LOCAL_CHECK_VERBOSE"

  print_version rustc
  print_version cargo

  step "Install greentic-dev"
  run_or_skip "greentic-dev install" install_greentic_dev
  if ! need greentic-component; then
    step "Install greentic-component (not provided by greentic-dev?)"
    run_or_skip "greentic-component install" bash -c "cargo binstall greentic-component -y"
  fi

  step "Formatting"
  run_or_skip "cargo fmt" fmt_check

  step "Clippy"
  run_or_skip "cargo clippy" clippy_check

  step "Build"
  run_or_skip "cargo build" build_check

  step "Tests"
  run_or_skip "cargo test" test_check

  step "Builder demo determinism"
  run_or_skip "builder demo" builder_demo_check

  step "Packc builds canonical gtpack"
  run_or_skip "packc gtpack" packc_gtpack_check

  echo ""
  echo "✅ local checks completed"
}

can_reach_cratesio() {
  if command -v python3 >/dev/null 2>&1; then
    PYTHON=python3
  elif command -v python >/dev/null 2>&1; then
    PYTHON=python
  else
    return 1
  fi
  "$PYTHON" - <<'PY'
import socket
try:
    socket.getaddrinfo("index.crates.io", None)
    print("ok")
    raise SystemExit(0)
except socket.gaierror:
    raise SystemExit(1)
PY
}

main "$@"
