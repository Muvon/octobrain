#!/usr/bin/env bash
# Full LongMemEval pipeline: fetch → ingest → answer → score → report.
# All paths absolute so this works regardless of cwd.
set -euo pipefail

# Resolve directories.
BENCH_HOME="${BENCH_HOME:-/bench}"
BENCH_DATA_DIR="${BENCH_DATA_DIR:-/data/bench}"
BENCH_RESULTS_DIR="${BENCH_RESULTS_DIR:-/data/results}"
OCTOBRAIN_DATA_DIR="${OCTOBRAIN_DATA_DIR:-/data/octobrain}"

LONGMEMEVAL_VARIANT="${LONGMEMEVAL_VARIANT:-longmemeval_s}"
LONGMEMEVAL_DIR="${BENCH_DATA_DIR}/longmemeval"
DATA_FILE="${LONGMEMEVAL_DIR}/data/${LONGMEMEVAL_VARIANT}.json"

# Fail fast on missing config.
: "${AGENT_BASE_URL:?must be set in .env (e.g., https://ollama.cloud/v1)}"
: "${AGENT_API_KEY:?must be set in .env}"
: "${AGENT_MODEL:?must be set in .env}"
: "${JUDGE_MODEL:?must be set in .env}"
: "${OCTOBRAIN_EMBEDDING_MODEL:?must be set in .env (e.g., openai:text-embedding-3-small)}"
: "${OCTOBRAIN_EMBEDDING_API_KEY:?must be set in .env (the key for the embedding provider)}"

mkdir -p "${BENCH_RESULTS_DIR}"

# Resume mode: if RESUME_FROM points at a prior run directory, skip the
# expensive ingest+answer stages and only re-score the existing
# hypothesis.jsonl. Use this to recover a paid run where only stage 4
# (scoring) failed — saves the money already spent on embeddings + LLM.
if [[ -n "${RESUME_FROM:-}" ]]; then
  RUN_DIR="${RESUME_FROM}"
  if [[ ! -f "${RUN_DIR}/hypothesis.jsonl" ]]; then
    echo "ERROR: RESUME_FROM=${RUN_DIR} has no hypothesis.jsonl — nothing to score" >&2
    exit 1
  fi
  echo "==> RESUME: scoring existing hypothesis at ${RUN_DIR}"
  TIMESTAMP="$(basename "${RUN_DIR}" | sed 's/^longmemeval-//')"
else
  TIMESTAMP="$(date -u +%Y-%m-%dT%H-%M-%SZ)"
  RUN_DIR="${BENCH_RESULTS_DIR}/longmemeval-${TIMESTAMP}"
  mkdir -p "${RUN_DIR}"
fi

if [[ -z "${RESUME_FROM:-}" ]]; then
  # Fresh run: start from a clean octobrain DB. Persisting state between runs
  # would contaminate later runs with accumulated memories. Only octobrain's
  # data dir gets wiped — bench-data (datasets) is preserved.
  # OCTOBRAIN_DATA_DIR is the volume mount point itself — we can't remove
  # the dir, only its contents.
  echo "==> Wiping octobrain data dir (fresh DB per run)"
  find "${OCTOBRAIN_DATA_DIR}" -mindepth 1 -delete
fi

HYPOTHESIS_FILE="${RUN_DIR}/hypothesis.jsonl"
SCORE_FILE="${RUN_DIR}/score.json"
RUN_LOG="${RUN_DIR}/run.log"
META_FILE="${RUN_DIR}/meta.json"

echo "==> LongMemEval run ${TIMESTAMP}"
echo "    variant=${LONGMEMEVAL_VARIANT}"
echo "    agent=${AGENT_MODEL}, judge=${JUDGE_MODEL}"
echo "    out=${RUN_DIR}"
echo ""

# Record exact configuration used for this run for reproducibility.
cat >"${META_FILE}" <<META
{
  "timestamp_utc": "${TIMESTAMP}",
  "variant": "${LONGMEMEVAL_VARIANT}",
  "agent_model": "${AGENT_MODEL}",
  "agent_base_url": "${AGENT_BASE_URL}",
  "judge_model": "${JUDGE_MODEL}",
  "embedding_model": "${OCTOBRAIN_EMBEDDING_MODEL}",
  "embedding_base_url": "${OCTOBRAIN_EMBEDDING_BASE_URL:-default}",
  "octobrain_version": "$(octobrain --version 2>&1 || echo unknown)",
  "hyde_enabled": "${OCTOBRAIN_HYDE_ENABLED:-1}",
  "sleep_consolidation": "${OCTOBRAIN_SLEEP_CONSOLIDATION:-1}",
  "max_questions": "${MAX_QUESTIONS:-0}"
}
META

# Stage 1: fetch dataset (idempotent — skips if already cached).
echo "==> Stage 1/4: fetch dataset"
"${BENCH_HOME}/scripts/fetch_longmemeval.sh" 2>&1 | tee -a "${RUN_LOG}"
echo ""

# Stage 1.5: pre-flight checks. Validate that the scorer can import all its
# deps BEFORE we pay for ingest+answer. If anything is missing here, we want
# to know now, not after a 5+ minute run that ends in a scoring crash.
echo "==> Stage 1.5: pre-flight (validate scorer can run)"
# Install ONLY the specific missing deps the scorer needs. Never pip install
# upstream's requirements-lite.txt directly — it pins openai==1.35.1 which
# downgrades our 1.59.5 and breaks against our httpx==0.28.1 (removed proxies
# kwarg). Permanent fix is in benches/requirements.txt baking these at build
# time; this just catches the case where the image hasn't been rebuilt yet.
pip install --quiet --no-cache-dir 'backoff==2.2.1' 'numpy>=1.26' 'nltk>=3.9' \
  2>&1 | tee -a "${RUN_LOG}" || true
python3 -c "import backoff, numpy, tqdm, openai" || {
  echo "ERROR: scorer deps not importable — refusing to start ingest" >&2
  exit 1
}
echo "    pre-flight OK"
echo ""

if [[ -z "${RESUME_FROM:-}" ]]; then
  # Stage 2 + 3: ingest sessions into octobrain, answer questions.
  # A single Python process drives both since they share an octobrain instance.
  echo "==> Stage 2-3/4: ingest sessions + answer questions"
  python3 -m adapters.longmemeval_run \
    --data-file "${DATA_FILE}" \
    --octobrain-data-dir "${OCTOBRAIN_DATA_DIR}" \
    --hypothesis-file "${HYPOTHESIS_FILE}" \
    --run-log "${RUN_LOG}" \
    2>&1 | tee -a "${RUN_LOG}"
  echo ""
else
  echo "==> Stage 2-3/4: SKIPPED (resume mode — using existing hypothesis)"
  echo ""
fi

# Stage 4: score via upstream's evaluate_qa.py. The scorer uses the openai
# SDK and only accepts its hardcoded whitelist (gpt-4o, gpt-4o-mini,
# llama-3.1-70b-instruct) — so the judge hits OpenAI, not Ollama. JUDGE_API_KEY
# falls back to OCTOBRAIN_EMBEDDING_API_KEY (same OpenAI account). Subshell
# scopes the env so it doesn't leak.
echo "==> Stage 4/4: score (judge=${JUDGE_MODEL})"

JUDGE_KEY_RESOLVED="${JUDGE_API_KEY:-${OCTOBRAIN_EMBEDDING_API_KEY}}"
JUDGE_URL_RESOLVED="${JUDGE_BASE_URL:-https://api.openai.com/v1}"
if [[ -z "${JUDGE_KEY_RESOLVED}" ]]; then
  echo "ERROR: judge needs an API key. Set JUDGE_API_KEY in .env, or set" >&2
  echo "       OCTOBRAIN_EMBEDDING_API_KEY (same OpenAI key for both)." >&2
  exit 1
fi
(
  export OPENAI_API_KEY="${JUDGE_KEY_RESOLVED}"
  export OPENAI_BASE_URL="${JUDGE_URL_RESOLVED}"
  cd "${LONGMEMEVAL_DIR}/src/evaluation" 2>/dev/null || cd "${LONGMEMEVAL_DIR}"
  python3 evaluate_qa.py "${JUDGE_MODEL}" "${HYPOTHESIS_FILE}" "${DATA_FILE}"
) 2>&1 | tee -a "${RUN_LOG}" \
       | tee "${SCORE_FILE}.raw"
echo ""

# Distill scorer output into a clean JSON summary.
python3 "${BENCH_HOME}/scripts/summarize_score.py" \
  "${SCORE_FILE}.raw" "${META_FILE}" >"${SCORE_FILE}"

echo "==> Done. Summary:"
cat "${SCORE_FILE}"
echo ""
echo "Full artifacts: ${RUN_DIR}"
