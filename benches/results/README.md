# Benchmark Results

Each run writes a timestamped subdirectory here. Contents:

```
longmemeval-<UTC timestamp>/
├── meta.json          # exact config used (models, flags, octobrain version)
├── hypothesis.jsonl   # adapter output: {question_id, hypothesis} per line
├── score.json         # distilled metrics (overall + per-category)
└── run.log            # full pipeline log
```

Commit summarized score files (`score.json` + `meta.json`) when you want to
publish numbers; the full directory can stay local — it's gitignored by default.
