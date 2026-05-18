# Octobrain Benchmarks

Reproducible quality benchmarks for octobrain's memory architecture. Everything
runs in Docker — no host install, no Python on your machine. Bring your own
LLM (any OpenAI-compatible endpoint: Ollama Cloud, OpenAI, Together, Groq, etc).

## Status

| Benchmark | License | Status | Notes |
|---|---|---|---|
| [LongMemEval](https://github.com/xiaowu0162/longmemeval) | MIT | wired | 500 questions across 6 memory abilities |
| [Memora](https://github.com/geniesinc/Memora) | tbd | planned | "From Recall to Forgetting" — FAMA metric |
| [LoCoMo](https://github.com/snap-research/locomo) | research | planned | 35-session multi-modal benchmark |

## Quick start

```bash
cd benches

# 1. Configure your LLM endpoint.
cp .env.example .env
$EDITOR .env                # set OPENAI_BASE_URL, OPENAI_API_KEY, AGENT_MODEL, JUDGE_MODEL

# 2. Build the image once (downloads + compiles octobrain in release mode).
make build

# 3. Smoke test first (5 questions, finishes in minutes).
make smoke

# 4. Full run.
make longmemeval
```

Each run writes a timestamped directory under `benches/results/`:

```
results/longmemeval-2026-05-18T14-23-01Z/
├── meta.json          # exact config used (models, flags, octobrain version)
├── hypothesis.jsonl   # one answer per question, ready for upstream scorer
├── score.json         # parsed metrics: per-category + overall
└── run.log            # full pipeline log
```

## Configuration

All knobs live in `.env`:

```bash
# OpenAI-compatible endpoint. Examples:
#   Ollama Cloud:  https://ollama.example.com/v1
#   Local Ollama:  http://host.docker.internal:11434/v1
#   OpenAI:        https://api.openai.com/v1
#   Together:      https://api.together.xyz/v1
OPENAI_BASE_URL=https://ollama.example.com/v1
OPENAI_API_KEY=sk-...

# Model the agent uses to answer questions from retrieved memory.
AGENT_MODEL=llama3.3:70b

# Model the scorer uses to judge answer correctness.
# Quality of the judge bottlenecks the whole eval — pick a strong model.
JUDGE_MODEL=gpt-oss:120b

# Which LongMemEval variant.
LONGMEMEVAL_VARIANT=longmemeval_s   # _s, _m, _oracle

# Octobrain feature flags during the run.
OCTOBRAIN_HYDE_ENABLED=1
OCTOBRAIN_SLEEP_CONSOLIDATION=1

# Cap question count (0 = all). Set to 5–20 for smoke tests.
MAX_QUESTIONS=0
```

## How it works

1. **Build** — `Dockerfile` does a two-stage build: stage 1 compiles
   `octobrain` in release mode (default features: FastEmbed + HuggingFace
   embeddings; no API key needed for embedding). Stage 2 is a slim
   `python:3.11-slim` with the binary, adapters, and pinned Python deps.
2. **Fetch** — Clones `xiaowu0162/longmemeval` and pulls the dataset
   (idempotent — skipped on re-runs).
3. **Ingest** — For each instance in the dataset, the adapter spins up
   an isolated octobrain MCP server, feeds every chat turn in via
   `memorize`, then queries via `remember`.
4. **Answer** — Retrieved memories + question → your configured agent
   model → answer text.
5. **Score** — Runs upstream's `evaluate_qa.py` with your configured
   judge model. The OpenAI SDK respects `OPENAI_BASE_URL`, so the same
   endpoint serves both agent and judge.
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

- **`octobrain MCP server failed to come up`** — usually a port collision.
  Change `--bind` in `longmemeval_run.py` or kill the conflicting process.
- **`OPENAI_BASE_URL must be set`** — you forgot `.env`. `cp .env.example .env`
  and fill it in.
- **Judge returns nonsense / 0% scores** — your `JUDGE_MODEL` is too weak.
  Try a larger model (gpt-oss:120b on Ollama, gpt-4o on OpenAI).
- **`huggingface-cli` dataset download fails** — manual: download the
  `longmemeval_s.json` from upstream's release and drop it in
  `/data/bench/longmemeval/data/` (via `docker compose ... shell`).
