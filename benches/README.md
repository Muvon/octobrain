# Octobrain Benchmarks

Reproducible quality benchmarks for octobrain. Two kinds live here:
**retrieval-quality** (knowledge system) is native Rust, fully local, metric-only
— no Docker, no LLM (see [below](#retrieval-quality-knowledge-system)). The
**memory** benchmarks (LongMemEval etc.) are LLM-judged and run in Docker —
bring your own LLM (any OpenAI-compatible endpoint).

## Status

| Benchmark | License | Status | Notes |
|---|---|---|---|
| [BEIR retrieval](https://github.com/beir-cellar/beir) | Apache-2.0 | wired | Knowledge-system nDCG@10; local, no LLM, no Docker |
| [LongMemEval](https://github.com/xiaowu0162/longmemeval) | MIT | wired | 500 questions across 6 memory abilities |
| [Memora](https://github.com/geniesinc/Memora) | tbd | planned | "From Recall to Forgetting" — FAMA metric |
| [LoCoMo](https://github.com/snap-research/locomo) | research | planned | 35-session multi-modal benchmark |

## Retrieval quality (knowledge system)

Measures how well octobrain's knowledge retrieval ranks relevant passages on
standard [BEIR](https://github.com/beir-cellar/beir) datasets, reported as
**nDCG@10 / Recall@10 / Recall@100 / MRR@10** against the official qrels (metrics
reproduce `pytrec_eval`: linear gain, log2 discount, score-desc / docid-desc
ordering, graded relevance for NFCorpus). No LLM judge — just the embedder + the
real `KnowledgeStore` retrieval path. The same index is queried three ways:
`vector` (dense only), `hybrid` (BM25 + vector RRF), `hybrid+rerank` (cross-encoder).

```bash
cd benches
bash scripts/run_retrieval.sh                              # scifact + nfcorpus, bge-small
DATASETS="scifact" bash scripts/run_retrieval.sh          # one dataset
EMBED_MODEL="fastembed:nomic-ai/nomic-embed-text-v1.5" bash scripts/run_retrieval.sh
BEIR_MAX_QUERIES=50 bash scripts/run_retrieval.sh         # quick smoke
```

The harness builds `cargo run --release --features bench --bin beir_bench`,
downloads each BEIR zip into `benches/data/`, indexes the corpus **once** per
`(dataset, embedding model)` (cached under the system temp dir; `BEIR_FRESH=1`
to rebuild), and writes `results.jsonl` + per-scenario logs under
`benches/results/retrieval-<ts>/`. Env knobs: `BEIR_SCENARIOS` (subset of
`vector,hybrid,hybrid+rerank`), `BEIR_RERANK_DEPTH` (default 50), `RERANK_MODEL`.

Latest numbers (bge-small-en-v1.5, the harness default embedder) are in the top-level
[README Benchmarks section](../README.md#benchmarks).

### Findings

- **vector / hybrid validated**: dense-only nDCG@10 reproduces the embedder's
  published BEIR numbers (SciFact 0.722 vs 0.713, NFCorpus 0.341 vs 0.343), and
  hybrid (BM25 + vector RRF, the default) adds a consistent ~+2 nDCG, beating
  classic BM25 on both. The harness is trustworthy.
- **⚠️ reranker is a no-op (needs investigation)**: `hybrid+rerank` returned
  *byte-identical* metrics to `hybrid` for **two different** fastembed
  cross-encoders (`jina-reranker-v2-base-multilingual` and `bge-reranker-base`),
  with zero errors. octolib sorts results by score, so identical output across
  two models means the fastembed reranker path yields degenerate scores that
  never reorder. The default config **enables this reranker for memory and
  knowledge search**, so it is silently doing nothing — confirm with a focused
  octolib/fastembed-rs repro and fix (or switch reranker provider) before
  relying on reranking. Rerank is therefore *excluded* from the headline numbers.

## Quick start

```bash
cd benches

# 1. Configure your LLM endpoint.
cp .env.example .env
$EDITOR .env                # set AGENT_BASE_URL, AGENT_API_KEY, AGENT_MODEL, JUDGE_MODEL

# 2. Build the image once (downloads + compiles octobrain in release mode).
make build

# 3. Smoke test first (5 questions, finishes in minutes).
make smoke

# 4. Full run.
make longmemeval
```

Other targets: `make tune` (60-question slice spanning all 6 categories, for
retrieval-knob tuning), `make resume RUN=<results-dir>` (re-score an existing
`hypothesis.jsonl` without re-running ingest), `make nuke` (full reset — also
drops datasets and the octobrain DB volume).

Each run writes a timestamped directory under `benches/results/`:

```
results/longmemeval-2026-05-18T14-23-01Z/
├── meta.json          # exact config used (models, flags, octobrain version)
├── hypothesis.jsonl   # one answer per question, ready for upstream scorer
├── score.json         # parsed metrics: per-category + overall
└── run.log            # full pipeline log
```

## Configuration

All knobs live in `.env` (see `.env.example` for the annotated version):

```bash
# ─ Chat LLM: agent answers + judge scoring ────────────────────────────────
# Any OpenAI-compatible endpoint: Ollama Cloud, OpenAI, Together, Groq, etc.
AGENT_BASE_URL=https://ollama.example.com/v1
AGENT_API_KEY=sk-...

# Model the agent uses to answer questions from retrieved memory.
AGENT_MODEL=kimi-k2.6

# Model the scorer uses to judge answer correctness.
# Quality of the judge bottlenecks the whole eval — pick a strong model.
JUDGE_MODEL=gpt-4o-mini

# Judge endpoint and key — both default to OpenAI / the embedding key below.
# Override only to point the judge at a different OpenAI-compatible endpoint.
JUDGE_BASE_URL=
JUDGE_API_KEY=

# ─ Embedding: matches SOTA paper setups for apples-to-apples comparison ───
# Mem0, ENGRAM and the leaderboard SOTA holders all use OpenAI's
# text-embedding-3-small; use the same to compare against published numbers.
OCTOBRAIN_EMBEDDING_MODEL=openai:text-embedding-3-small
OCTOBRAIN_EMBEDDING_API_KEY=sk-...
OCTOBRAIN_EMBEDDING_BASE_URL=   # blank → provider default

# Which LongMemEval variant (filename without .json). Upstream's cleaned
# release ships three: longmemeval_oracle (no haystack — sanity check),
# longmemeval_s_cleaned (~115K tokens/instance, cheapest),
# longmemeval_m_cleaned (~500 sessions, larger).
LONGMEMEVAL_VARIANT=longmemeval_s_cleaned

# Recorded in meta.json for provenance only. The adapter writes its own
# octobrain config.toml (HyDE on, reranker off) — that file controls behavior.
OCTOBRAIN_HYDE_ENABLED=1
OCTOBRAIN_SLEEP_CONSOLIDATION=1

# Cap question count (0 = all). Set to 5–20 for smoke tests.
MAX_QUESTIONS=0
```

## How it works

1. **Build** — `Dockerfile` does a two-stage build: stage 1 compiles
   `octobrain` in release mode (default features: FastEmbed + HuggingFace
   embeddings; no API key needed for embedding). Stage 2 is a slim
   `python:3.11-slim-trixie` with the binary, adapters, and pinned Python deps.
2. **Fetch** — Clones `xiaowu0162/longmemeval` and pulls the dataset
   (idempotent — skipped on re-runs).
3. **Ingest** — For each instance in the dataset, the adapter spins up
   an isolated octobrain MCP server, feeds every chat turn in via
   `memorize`, then queries via `remember`.
4. **Answer** — Retrieved memories + question → your configured agent
   model → answer text.
5. **Score** — Runs upstream's `evaluate_qa.py` with your configured
   judge model. The judge hits `JUDGE_BASE_URL` (default: OpenAI) and falls
   back to `OCTOBRAIN_EMBEDDING_API_KEY` when `JUDGE_API_KEY` is unset.
6. **Report** — Distilled JSON summary printed at the end + persisted
   alongside the raw artifacts.

## Reproducibility guarantees

- `octobrain` is pinned to the exact source tree at build time.
- Python deps are pinned in `requirements.txt`.
- LongMemEval source is checked out at a pinned commit (`LONGMEMEVAL_COMMIT`
  env, defaults to `main` — pin a SHA for archival runs).
- The full configuration is recorded in `meta.json` next to every result.
- Two runs with identical `.env` produce identical hypotheses (assuming
  the LLM endpoint is deterministic at `temperature=0.0`).

## Cost estimate

LongMemEval-S on Ollama Cloud at typical pricing:
- ~500 questions × ~10K input + 400 output tokens = ~5M agent tokens
- ~500 judge calls × similar size = ~5M judge tokens
- Total ≈ **10M tokens, $1–10** depending on Ollama Cloud tier

OpenAI GPT-4o equivalent: ~$50–100 for the full run.

## Adding a benchmark

The pattern is small:

1. New adapter under `adapters/<bench>_run.py` that ingests its data
   format into octobrain and writes hypotheses in the upstream scorer's
   format.
2. New script under `scripts/run_<bench>.sh` that orchestrates fetch +
   ingest + score.
3. New Makefile target.
4. New case in `scripts/entrypoint.sh`.

## Troubleshooting

- **`AGENT_BASE_URL must be set` / `<MODEL> must be set`** — you forgot `.env`.
  `cp .env.example .env` and fill it in. The adapter drives octobrain over
  stdio (no port binding), so a hung run is a crashed child process, not a port
  collision — check `run.log`.
- **Judge returns nonsense / 0% scores** — your `JUDGE_MODEL` is too weak.
  Try a larger model (gpt-4o, or a strong model on your endpoint).
- **Dataset download fails** — the fetch script pulls the three cleaned
  variants from HuggingFace over `curl`; if it can't reach HF, download
  `longmemeval_<variant>.json` manually and drop it in
  `/data/bench/longmemeval/data/` (via `docker compose ... shell`).
