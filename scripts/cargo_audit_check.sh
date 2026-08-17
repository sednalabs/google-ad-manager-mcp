#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IGNORED_ADVISORY="RUSTSEC-2026-0235"
INACTIVE_PACKAGE="rkyv@0.7.46"

cd "${ROOT_DIR}"

# rust_decimal declares rkyv 0.7 as an optional dependency, so Cargo.lock must
# carry it even though GAM does not enable that feature. Keep the RustSec
# exception valid only while the vulnerable package is absent from every
# normal/build target graph.
active_tree="$(cargo tree --locked --target all --edges normal,build -i "${INACTIVE_PACKAGE}")"
if [[ -n "${active_tree}" ]]; then
  echo "${IGNORED_ADVISORY} exception is no longer valid: ${INACTIVE_PACKAGE} is active" >&2
  printf '%s\n' "${active_tree}" >&2
  exit 1
fi

cargo audit --deny warnings --ignore "${IGNORED_ADVISORY}"
