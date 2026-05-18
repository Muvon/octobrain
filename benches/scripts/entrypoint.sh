#!/usr/bin/env bash
# Single entrypoint for the bench container. Dispatches to the requested target.
# Usage (inside container): entrypoint.sh <longmemeval|memora|smoke|shell>
set -euo pipefail

TARGET="${1:-longmemeval}"

case "${TARGET}" in
  longmemeval)
    exec /bench/scripts/run_longmemeval.sh
    ;;
  memora)
    echo "memora pipeline: not yet wired" >&2
    exit 64
    ;;
  shell|bash)
    exec /bin/bash
    ;;
  *)
    echo "unknown target: ${TARGET}" >&2
    echo "valid: longmemeval | memora | shell" >&2
    exit 64
    ;;
esac
