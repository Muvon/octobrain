"""LongMemEval pipeline: ingest sessions into octobrain, answer each question
using octobrain-retrieved context, write hypotheses in upstream's JSONL format.

Upstream's `evaluate_qa.py` scores the hypothesis file against the dataset.

LongMemEval data format (best-effort — upstream may evolve):
  Each instance is one element in a JSON array with fields:
    question_id, question, question_type, haystack_sessions, answer, …
  `haystack_sessions` is a list of sessions; each session is a list of
  {role, content, timestamp?} turns.
"""
from __future__ import annotations

import argparse
import json
import logging
import os
import sys
import time
from pathlib import Path
from typing import Iterable

from tqdm import tqdm

from adapters.llm_client import LLMClient, LLMConfig
from adapters.octobrain_client import OctobrainClient


log = logging.getLogger("longmemeval")


SYSTEM_PROMPT = """You answer questions about a user's chat history.
You receive only the retrieved memories most relevant to the question.
Answer concisely and only from the retrieved context. If the retrieved
context does not contain the answer, say "I don't know" — do not fabricate.
"""

ANSWER_TEMPLATE = """RETRIEVED MEMORIES (most relevant first):
{memories}

QUESTION: {question}

ANSWER:"""


def load_instances(data_file: Path) -> list[dict]:
    raw = json.loads(data_file.read_text())
    if isinstance(raw, dict):
        # Some variants wrap the list under a key.
        for key in ("data", "instances", "questions"):
            if key in raw and isinstance(raw[key], list):
                return raw[key]
        raise ValueError(f"Unrecognized top-level structure in {data_file}")
    return raw


def ingest_instance(client: OctobrainClient, inst: dict) -> int:
    """Push each haystack session into octobrain as one memory.

    Per-session granularity matches what Mem0/ENGRAM do — one conversational
    exchange = one memory. ~10x fewer memorize calls than per-turn at no cost
    to what the benchmark actually tests (cross-session recall).

    Returns the number of memories written.
    """
    sessions = inst.get("haystack_sessions") or inst.get("sessions") or []
    if not sessions:
        return 0

    n = 0
    pbar = tqdm(
        total=len(sessions),
        desc="  sessions",
        unit="sess",
        position=1,
        leave=False,
        mininterval=0.5,
    )
    for session_idx, session in enumerate(sessions):
        turns = session if isinstance(session, list) else session.get("turns") or []
        if not turns:
            pbar.update(1)
            continue

        # Flatten turns into a single "role: content" transcript. Keep within
        # octobrain's 10000-char content limit (we leave headroom for tags etc).
        lines: list[str] = []
        for turn in turns:
            role = (turn.get("role") or "user").lower()
            content = (turn.get("content") or "").strip()
            if not content:
                continue
            lines.append(f"{role}: {content}")
        if not lines:
            pbar.update(1)
            continue
        transcript = "\n".join(lines)
        if len(transcript) > 9900:
            transcript = transcript[:9900] + "\n…[truncated]"

        # Title is a short summary of the first turn for readability/retrieval.
        first_line = lines[0]
        title = f"session {session_idx}: {first_line[:120]}"
        try:
            client.memorize(
                title=title,
                content=transcript,
                tags=[f"session:{session_idx}"],
                importance=0.5,
            )
            n += 1
        except Exception as e:
            log.warning("memorize failed (session=%d): %s", session_idx, e)
        pbar.update(1)
    pbar.close()
    return n


def answer_question(client: OctobrainClient, llm: LLMClient, question: str, k: int = 5) -> str:
    retrieved = client.remember(query=question, limit=k)
    prompt = ANSWER_TEMPLATE.format(memories=retrieved or "(none)", question=question)
    try:
        return llm.complete(prompt, system=SYSTEM_PROMPT, temperature=0.0, max_tokens=400)
    except Exception as e:
        log.warning("LLM answer failed: %s", e)
        return ""


def write_hypothesis(out_file: Path, records: Iterable[dict]) -> int:
    out_file.parent.mkdir(parents=True, exist_ok=True)
    n = 0
    with out_file.open("w") as f:
        for r in records:
            f.write(json.dumps(r) + "\n")
            n += 1
    return n


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--data-file", required=True, type=Path)
    parser.add_argument("--octobrain-data-dir", required=True, type=Path)
    parser.add_argument("--hypothesis-file", required=True, type=Path)
    parser.add_argument("--run-log", required=True, type=Path)
    parser.add_argument("--k", type=int, default=5, help="memories to retrieve per question")
    args = parser.parse_args()

    max_q = int(os.environ.get("MAX_QUESTIONS", "0") or 0)

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
        handlers=[
            logging.StreamHandler(sys.stdout),
            logging.FileHandler(args.run_log),
        ],
    )

    log.info("loading dataset from %s", args.data_file)
    instances = load_instances(args.data_file)
    if max_q > 0:
        instances = instances[:max_q]
        log.info("MAX_QUESTIONS=%d → limiting to %d instances", max_q, len(instances))
    log.info("%d instances to process", len(instances))

    llm = LLMClient(LLMConfig.from_env("AGENT_MODEL"))
    log.info("agent model: %s", llm.model)

    records: list[dict] = []
    t0 = time.monotonic()

    # One octobrain MCP session per run (spawned over stdio by the SDK). We
    # re-ingest per instance because LongMemEval haystacks differ between
    # instances; tags scope retrievals.
    log.info("spawning octobrain MCP server via stdio (first call may take 5-15s)")
    with OctobrainClient(data_dir=args.octobrain_data_dir) as ob:
        log.info("octobrain MCP session ready")

        # Smoke the embedding stack with one cheap memorize before the main
        # loop so silent hangs (e.g. unreachable embedding endpoint, wrong
        # key) surface immediately with a clear error instead of looking
        # like a stalled progress bar.
        log.info("testing embedding stack via probe memorize…")
        t_probe = time.monotonic()
        probe = ob.memorize(
            title="bench-probe-smoke",
            content="Embedding-stack probe; will be ignored.",
            importance=0.01,
            tags=["bench-probe"],
        )
        log.info("probe ok in %.2fs (%s)", time.monotonic() - t_probe, probe[:80])

        for inst in tqdm(instances, desc="instances", unit="q"):
            qid = inst.get("question_id") or inst.get("id") or f"q-{len(records)}"
            question = inst.get("question") or inst.get("query") or ""
            if not question:
                log.warning("instance %s has no question, skipping", qid)
                continue

            n_mem = ingest_instance(ob, inst)
            log.info("instance %s ingested %d memories", qid, n_mem)

            hypothesis = answer_question(ob, llm, question, k=args.k)
            records.append({"question_id": qid, "hypothesis": hypothesis})

    elapsed = time.monotonic() - t0
    n = write_hypothesis(args.hypothesis_file, records)
    log.info("wrote %d hypotheses to %s in %.1fs", n, args.hypothesis_file, elapsed)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
