#!/usr/bin/env bash
#
# Local, no-LLM retrieval-quality benchmark for octobrain's knowledge system.
# Downloads small BEIR datasets, then runs the REAL KnowledgeStore retrieval
# path (vector / hybrid / hybrid+rerank) over them and reports nDCG@10 /
# Recall@k / MRR@10 against qrels. Each scenario is just a config file fed via
# OCTOBRAIN_CONFIG_PATH, generated from config-templates/default.toml so there
# is a single source of truth for the non-varying knobs.
#
# Usage:
#   benches/scripts/run_retrieval.sh                 # default: scifact + nfcorpus, bge-small trio
#   DATASETS="scifact" benches/scripts/run_retrieval.sh
#   EMBED_MODEL="fastembed:nomic-ai/nomic-embed-text-v1.5" benches/scripts/run_retrieval.sh
#   BEIR_MAX_QUERIES=50 benches/scripts/run_retrieval.sh   # quick smoke
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$BENCH_DIR/.." && pwd)"
TEMPLATE="$REPO_DIR/config-templates/default.toml"
DATA_DIR="$BENCH_DIR/data"

DATASETS="${DATASETS:-scifact nfcorpus}"
EMBED_MODEL="${EMBED_MODEL:-fastembed:BAAI/bge-small-en-v1.5}"
RERANK_MODEL="${RERANK_MODEL:-fastembed:jina-reranker-v2-base-multilingual}"
BEIR_BASE_URL="https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets"

BIN="$REPO_DIR/target/release/beir_bench"
TS="$(date -u +%Y-%m-%dT%H-%M-%SZ)"
OUT_DIR="$BENCH_DIR/results/retrieval-$TS"
mkdir -p "$OUT_DIR" "$DATA_DIR"
RESULTS="$OUT_DIR/results.jsonl"

# Build once if needed.
if [ ! -x "$BIN" ]; then
  echo "[run] building beir_bench (release)…" >&2
  ( cd "$REPO_DIR" && cargo build --release --features bench --bin beir_bench )
fi

# Fetch a BEIR dataset zip if not already present.
fetch() {
  local ds="$1"
  if [ ! -d "$DATA_DIR/$ds" ]; then
    echo "[run] downloading $ds…" >&2
    curl -sSL "$BEIR_BASE_URL/$ds.zip" -o "$DATA_DIR/$ds.zip"
    unzip -q -o "$DATA_DIR/$ds.zip" -d "$DATA_DIR"
    rm -f "$DATA_DIR/$ds.zip"
  fi
}

# Generate a scenario config from the template, overriding only the knobs that
# vary. awk tracks the current [section] so it edits the right `enabled`/`model`.
gen_config() {
  local out="$1" embed="$2" hybrid="$3" rerank="$4" rerank_model="$5"
  awk -v emb="$embed" -v hyb="$hybrid" -v rrk="$rerank" -v rrkm="$rerank_model" '
    /^\[/ { sect=$0 }
    sect=="[embedding]" && /^model =/ { print "model = \"" emb "\""; next }
    sect=="[search.hybrid]" && /^enabled =/ { print "enabled = " hyb; next }
    sect=="[search.reranker]" && /^enabled =/ { print "enabled = " rrk; next }
    sect=="[search.reranker]" && /^model =/ { print "model = \"" rrkm "\""; next }
    { print }
  ' "$TEMPLATE" > "$out"
}

echo "[run] embedding: $EMBED_MODEL" >&2
echo "[run] datasets:  $DATASETS" >&2
: > "$RESULTS"

# One config per embedding model — the bench indexes each corpus once and sweeps
# vector / hybrid / hybrid+rerank internally, emitting one JSON line per scenario.
cfg="$OUT_DIR/config.toml"
gen_config "$cfg" "$EMBED_MODEL" "true" "true" "$RERANK_MODEL"

for ds in $DATASETS; do
  fetch "$ds"
  echo "[run] === $ds ===" >&2
  OCTOBRAIN_CONFIG_PATH="$cfg" "$BIN" "$DATA_DIR/$ds" "$ds" \
    | tee "$OUT_DIR/log-$ds.txt" \
    | grep '^JSON ' | sed 's/^JSON //' >> "$RESULTS" || true
done

echo >&2
echo "[run] results → $RESULTS" >&2
# Compact table to stdout.
python3 - "$RESULTS" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
if not rows:
    print("no results"); sys.exit(0)
w = max(len(r["label"]) if "label" in r else 0 for r in rows)
hdr = f'{"label".ljust(w)}  {"nDCG@10":>8} {"R@10":>7} {"R@100":>7} {"MRR@10":>7}  scenario'
print(hdr); print("-"*len(hdr))
for r in rows:
    print(f'{r["label"].ljust(w)}  {r["ndcg@10"]:>8.4f} {r["recall@10"]:>7.4f} '
          f'{r["recall@100"]:>7.4f} {r["mrr@10"]:>7.4f}  hybrid={r["hybrid"]} rerank={r["reranker"]}')
PY
