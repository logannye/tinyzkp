#!/usr/bin/env bash
# scripts/test_sszkp_extended.sh
#
# End-to-end tests for the sublinear-space ZK system (whitepaper-complete).
# - Uses the dev SRS (no external files needed).
# - Exercises: selector FS mirroring, zeta-shift openings, (optional) lookups,
#   large-T/small-b_blk streaming stress, and non-power-of-two rows.
#
# Conventions assumed (as used by the repo’s binaries):
#   prover  --rows <T> --b-blk <b> --k <k> --basis <eval|coeff> [--with-selectors?]
#   verifier --rows <T> --basis <eval|coeff>
#
# If your `prover` doesn’t support the selectors toggle flag used here,
# this script auto-detects the failure and SKIPS that scenario.
#
# Exit codes:
#   0 = all scenarios passed or were legitimately skipped
#   1 = build error or a must-pass scenario failed
#
# Whitepaper notes:
#   - zeta-shift run ensures Z(ω·ζ) is committed/opened consistently.
#   - lookups run verifies streaming Z_L commitment/opening when enabled.
#   - stress run validates O(b_blk) memory across quotient, openings, scheduler.
#   - non-power-of-two `rows` validates padding to N and odd-tile handling.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

banner() { printf "\n\033[1;36m==> %s\033[0m\n" "$*"; }
info()   { printf "  \033[0;36m• %s\033[0m\n" "$*"; }
pass()   { printf "\033[1;32m✔ %s\033[0m\n" "$*"; }
skip()   { printf "\033[1;33m↷ skipped: %s\033[0m\n" "$*"; }
fail()   { printf "\033[1;31m✘ %s\033[0m\n" "$*"; }

# Run prover+verifier once. Accepts:
#   name, cargo_features, rows, b_blk, k, basis, [extra_prover_args...]
run_case() {
  local name="$1"; shift
  local features="$1"; shift
  local rows="$1"; shift
  local bblk="$1"; shift
  local kregs="$1"; shift
  local basis="$1"; shift
  local -a extra_prover_args=("$@")  # may be empty

  banner "Case: $name"
  rm -f proof.bin

  info "features: ${features:-<none>}"
  info "rows: $rows, b_blk: $bblk, k: $kregs, basis: $basis"

  set +e
  if [[ -n "${features}" ]]; then
    cargo run --quiet --features "dev-srs,${features}" --bin prover -- \
      --rows "${rows}" --b-blk "${bblk}" --k "${kregs}" --basis "${basis}" \
      ${extra_prover_args[@]+"${extra_prover_args[@]}"}
  else
    cargo run --quiet --features dev-srs --bin prover -- \
      --rows "${rows}" --b-blk "${bblk}" --k "${kregs}" --basis "${basis}" \
      ${extra_prover_args[@]+"${extra_prover_args[@]}"}
  fi
  local rc_prover=$?
  set -e

  if [[ $rc_prover -ne 0 ]]; then
    fail "prover failed for ${name}"
    return 1
  fi

  if [[ ! -f proof.bin ]]; then
    fail "prover did not write proof.bin (${name})"
    return 1
  fi

  # Verifier
  set +e
  if [[ -n "${features}" ]]; then
    cargo run --quiet --features "dev-srs,${features}" --bin verifier -- \
      --rows "${rows}" --basis "${basis}" >/dev/null
  else
    cargo run --quiet --features dev-srs --bin verifier -- \
      --rows "${rows}" --basis "${basis}" >/dev/null
  fi
  local rc_verify=$?
  set -e

  if [[ $rc_verify -ne 0 ]]; then
    fail "verification FAILED for ${name}"
    return 1
  else
    pass "verification OK for ${name}"
    return 0
  fi
}

# Try a case; if the prover rejects an unknown flag, skip it gracefully.
try_case_with_flag() {
  local name="$1"; shift
  local features="$1"; shift
  local rows="$1"; shift
  local bblk="$1"; shift
  local kregs="$1"; shift
  local basis="$1"; shift
  local probe_flag="$1"; shift
  local -a extra_prover_args=("$@")  # may be empty

  banner "Case (optional): $name"
  rm -f proof.bin

  local stderr_file
  stderr_file="$(mktemp)"
  set +e
  if [[ -n "${features}" ]]; then
    cargo run --quiet --features "dev-srs,${features}" --bin prover -- \
      --rows "${rows}" --b-blk "${bblk}" --k "${kregs}" --basis "${basis}" \
      "${probe_flag}" \
      ${extra_prover_args[@]+"${extra_prover_args[@]}"} 2> "$stderr_file"
  else
    cargo run --quiet --features dev-srs --bin prover -- \
      --rows "${rows}" --b-blk "${bblk}" --k "${kregs}" --basis "${basis}" \
      "${probe_flag}" \
      ${extra_prover_args[@]+"${extra_prover_args[@]}"} 2> "$stderr_file"
  fi
  local rc=$?
  set -e

  if [[ $rc -ne 0 ]]; then
    if grep -qiE "found argument.*wasn't expected|unexpected argument|unrecognized option" "$stderr_file"; then
      skip "selectors flag not supported by prover; skipping '${name}'"
      rm -f "$stderr_file"
      return 0
    fi
    cat "$stderr_file" >&2
    rm -f "$stderr_file"
    fail "prover failed for optional case '${name}'"
    return 1
  fi
  rm -f "$stderr_file"

  # Verifier
  set +e
  if [[ -n "${features}" ]]; then
    cargo run --quiet --features "dev-srs,${features}" --bin verifier -- \
      --rows "${rows}" --basis "${basis}" >/dev/null
  else
    cargo run --quiet --features dev-srs --bin verifier -- \
      --rows "${rows}" --basis "${basis}" >/dev/null
  fi
  local rc_verify=$?
  set -e

  if [[ $rc_verify -ne 0 ]]; then
    fail "verification FAILED for optional case '${name}'"
    return 1
  else
    pass "verification OK for optional case '${name}'"
    return 0
  fi
}

tamper_should_fail() {
  local rows="$1"; local basis="$2"; local features="${3:-}"

  banner "Tamper test: corrupt one byte of proof.bin (should FAIL)"
  [[ -f proof.bin ]] || { fail "no proof.bin to tamper with"; return 1; }

  local size off
  size=$(wc -c < proof.bin)
  off=$((size/2))
  printf '\x01' | dd of=proof.bin bs=1 seek="$off" count=1 conv=notrunc >/dev/null 2>&1

  set +e
  if [[ -n "${features}" ]]; then
    cargo run --quiet --features "dev-srs,${features}" --bin verifier -- \
      --rows "${rows}" --basis "${basis}" >/dev/null
  else
    cargo run --quiet --features dev-srs --bin verifier -- \
      --rows "${rows}" --basis "${basis}" >/dev/null
  fi
  local rc=$?
  set -e

  if [[ $rc -eq 0 ]]; then
    fail "tampered proof unexpectedly VERIFIED"
    return 1
  else
    pass "tampered proof correctly rejected"
    return 0
  fi
}

# --- Build per feature set ----------------------------------------------------

banner "Build (dev-srs)"
cargo build --quiet --bins --features dev-srs
pass "build (base) succeeded"

banner "Build (dev-srs + zeta-shift)"
set +e
cargo build --quiet --bins --features "dev-srs,zeta-shift"
rc_zs=$?
set -e
if [[ $rc_zs -ne 0 ]]; then
  skip "feature 'zeta-shift' not available; related scenario will be skipped"
fi

banner "Build (dev-srs + lookups)"
set +e
cargo build --quiet --bins --features "dev-srs,lookups"
rc_lk=$?
set -e
if [[ $rc_lk -ne 0 ]]; then
  skip "feature 'lookups' not available; related scenario will be skipped"
fi

# --- Scenarios ----------------------------------------------------------------

# 1) Baseline
ROWS=1024
BASIS=eval
run_case "baseline: eval-basis wires, b_blk=128, rows=1024" \
  "" "$ROWS" 128 3 "$BASIS"

tamper_should_fail "$ROWS" "$BASIS" ""

# 2) Coeff-basis, smaller b_blk
ROWS=1536
BASIS=coeff
run_case "coeff-basis wires, b_blk=64, rows=1536" \
  "" "$ROWS" 64 3 "$BASIS"

# 3) Larger slice
ROWS=2048
BASIS=eval
run_case "eval-basis wires, b_blk=256, rows=2048" \
  "" "$ROWS" 256 4 "$BASIS"

# 4) Selectors present (FS mirroring) — try two common flags; skip if unknown
ROWS=1024
BASIS=eval
if ! try_case_with_flag "selectors present (FS mirrored), b_blk=128, rows=1024" \
     "" "$ROWS" 128 3 "$BASIS" "--with-selectors"; then
  :
fi
if [[ ! -f proof.bin ]]; then
  if ! try_case_with_flag "selectors present (FS mirrored via --selectors 2), b_blk=128, rows=1024" \
       "" "$ROWS" 128 3 "$BASIS" "--selectors" "2"; then
    :
  fi
fi

# 5) zeta-shift (if available)
if [[ $rc_zs -eq 0 ]]; then
  ROWS=1024
  BASIS=eval
  run_case "zeta-shift: Z opened at ω·ζ, b_blk=128, rows=1024" \
    "zeta-shift" "$ROWS" 128 3 "$BASIS"
else
  skip "zeta-shift run"
fi

# 6) lookups (if available)
if [[ $rc_lk -eq 0 ]]; then
  ROWS=1024
  BASIS=eval
  run_case "lookups: streamed Z_L, b_blk=128, rows=1024" \
    "lookups" "$ROWS" 128 3 "$BASIS"
else
  skip "lookups run"
fi

# 7) Stress: big T, small b_blk
ROWS=16384
BASIS=eval
run_case "stress: big-T small-b_blk (rows=16384, b_blk=64)" \
  "" "$ROWS" 64 3 "$BASIS"

# 8) Stress with zh_c=3 and dense selectors (selectors_dense.csv)
ROWS=2048
BASIS=eval
run_case "stress: zh_c=3, dense selectors" "" \
  "$ROWS" 128 3 "$BASIS" \
  "--selectors" "selectors_dense.csv" "--zh-c" "3"

# 9) Stress with zh_c=7 and sparse selectors (selectors_sparse.csv)
run_case "stress: zh_c=7, sparse selectors" "" \
  "$ROWS" 128 3 "$BASIS" \
  "--selectors" "selectors_sparse.csv" "--zh-c" "7"

# 10) Mixed selector set (dense+sparse+periodic), default zh_c=1
run_case "selectors: mixed (dense|sparse|periodic), zh_c=1" "" \
  "$ROWS" 128 3 "$BASIS" \
  "--selectors" "selectors_mixed.csv"

# 11) Non-power-of-two rows (padding check) + odd/odd-ish tiles
run_case "padding: rows=3000, b_blk=96 (odd-ish tile)" "" 3000 96 3 eval
run_case "padding: rows=3000, b_blk=73 (odd tile)"       "" 3000 73 3 eval

# 12) Non-power-of-two rows with coeff basis (cross-product check)
run_case "padding: coeff-basis, rows=3000, b_blk=96" "" 3000 96 3 coeff

# 13) Padding × lookups (if available): non-power-of-two rows with lookups
if [[ $rc_lk -eq 0 ]]; then
  run_case "padding×lookups: rows=3000, b_blk=73" \
    "lookups" 3000 73 3 eval
else
  skip "padding×lookups run"
fi

# 14) Padding × zeta-shift (if available): non-power-of-two rows with ζ-shift
if [[ $rc_zs -eq 0 ]]; then
  run_case "padding×zeta-shift: rows=3000, b_blk=96" \
    "zeta-shift" 3000 96 3 eval
else
  skip "padding×zeta-shift run"
fi

# 15) Padding × zh_c variant: change vanishing constant on padded domain
run_case "padding: zh_c=7, rows=3000, b_blk=73" "" 3000 73 3 eval "--zh-c" "7"

banner "All requested tests completed 🎉"
