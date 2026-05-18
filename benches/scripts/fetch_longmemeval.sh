#!/usr/bin/env bash
# Fetch LongMemEval source repo + dataset into the persistent bench-data volume.
# Idempotent: skips fetch when artifacts already exist. Pinned to a known good commit.
#
# Upstream hosts the cleaned dataset on HF at xiaowu0162/longmemeval-cleaned with
# three files: longmemeval_oracle.json, longmemeval_s_cleaned.json,
# longmemeval_m_cleaned.json. We pull all three (cheap, shared volume) so the
# operator can switch LONGMEMEVAL_VARIANT in .env without re-fetching.
set -euo pipefail

LONGMEMEVAL_REPO="${LONGMEMEVAL_REPO:-https://github.com/xiaowu0162/longmemeval.git}"
LONGMEMEVAL_COMMIT="${LONGMEMEVAL_COMMIT:-main}"
LONGMEMEVAL_VARIANT="${LONGMEMEVAL_VARIANT:-longmemeval_s_cleaned}"

HF_BASE="https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main"
DATA_FILES=(
  "longmemeval_oracle.json"
  "longmemeval_s_cleaned.json"
  "longmemeval_m_cleaned.json"
)

BENCH_DIR="${BENCH_DATA_DIR:-/data/bench}/longmemeval"
DATA_DIR="${BENCH_DIR}/data"
DATA_FILE="${DATA_DIR}/${LONGMEMEVAL_VARIANT}.json"

mkdir -p "${DATA_DIR}"

if [[ ! -d "${BENCH_DIR}/.git" ]]; then
  echo "==> Cloning LongMemEval repo to ${BENCH_DIR}"
  git clone "${LONGMEMEVAL_REPO}" "${BENCH_DIR}"
  git -C "${BENCH_DIR}" checkout "${LONGMEMEVAL_COMMIT}"
else
  echo "==> LongMemEval repo already present at ${BENCH_DIR}"
fi

for fname in "${DATA_FILES[@]}"; do
  target="${DATA_DIR}/${fname}"
  if [[ -s "${target}" ]]; then
    echo "==> Already cached: ${fname}"
    continue
  fi
  echo "==> Downloading ${fname} from HuggingFace…"
  if ! curl -fL --retry 5 --retry-delay 3 \
        -o "${target}.tmp" "${HF_BASE}/${fname}"; then
    rm -f "${target}.tmp"
    echo "WARN: failed to download ${fname} — continuing (manual drop possible)" >&2
    continue
  fi
  mv "${target}.tmp" "${target}"
  echo "    -> $(du -h "${target}" | cut -f1)"
done

if [[ ! -s "${DATA_FILE}" ]]; then
  echo "ERROR: required variant '${LONGMEMEVAL_VARIANT}' missing at ${DATA_FILE}." >&2
  echo "       Available files in ${DATA_DIR}:" >&2
  ls -la "${DATA_DIR}" >&2 || true
  echo "       Drop ${LONGMEMEVAL_VARIANT}.json into ${DATA_DIR}/ and re-run." >&2
  exit 1
fi

echo "==> Dataset ready: ${DATA_FILE}"
