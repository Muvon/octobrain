#!/usr/bin/env python3
"""Distill LongMemEval evaluator output into a compact JSON summary.

Usage: summarize_score.py <raw_score_file> <meta_file>
"""
from __future__ import annotations
import json
import re
import sys
from pathlib import Path


def parse_raw_scores(raw_text: str) -> dict[str, float]:
    """LongMemEval prints lines like:
        single-session-user: 0.842
        multi-session: 0.717
        overall: 0.756
    Best-effort extraction — upstream format may evolve.
    """
    scores: dict[str, float] = {}
    for line in raw_text.splitlines():
        m = re.match(r"^\s*([a-zA-Z][a-zA-Z0-9_\-]+)\s*:\s*([0-9.]+)\s*$", line)
        if m:
            key, val = m.group(1), m.group(2)
            try:
                scores[key] = float(val)
            except ValueError:
                pass
    return scores


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 64

    raw_path = Path(sys.argv[1])
    meta_path = Path(sys.argv[2])

    raw_text = raw_path.read_text() if raw_path.exists() else ""
    meta = json.loads(meta_path.read_text()) if meta_path.exists() else {}

    scores = parse_raw_scores(raw_text)

    summary = {
        "meta": meta,
        "scores": scores,
        "scorer_output_excerpt": raw_text[-2000:] if len(raw_text) > 2000 else raw_text,
    }
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
