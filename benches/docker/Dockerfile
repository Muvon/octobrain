# Two-stage build: compile octobrain natively, then run benchmarks in a Python image
# carrying the pre-built binary. Keeps the runtime image lean and reproducible.

# ─── Stage 1: build octobrain ─────────────────────────────────────────────────
# Trixie (glibc 2.41+) — Bookworm's glibc 2.36 lacks the ISO C23 strtoll/strtoul
# symbols that ort.pyke.io's prebuilt ONNX Runtime is linked against, causing
# link-time failures on octobrain's release build with default (fastembed +
# huggingface) features. Trixie matches what the prebuilt expects.
FROM rust:1.95-trixie AS rust-builder

RUN apt-get update -qq && \
    apt-get install -y -qq --no-install-recommends \
        protobuf-compiler libprotobuf-dev \
        cmake pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /src
# Copy the whole repo — Cargo.toml + Cargo.lock + src/.
# Build context is set to the repo root in docker-compose; see that file.
COPY . .

# Build with default features (fastembed + huggingface) so embeddings work
# offline without API keys. Release mode for representative latency.
RUN cargo build --release --bin octobrain

# ─── Stage 2: benchmark runtime ───────────────────────────────────────────────
# Trixie too — must match the builder's glibc for the binary to load the ort
# prebuilt at runtime.
FROM python:3.11-slim-trixie

ENV PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    OCTOBRAIN_DATA_DIR=/data/octobrain \
    BENCH_DATA_DIR=/data/bench \
    BENCH_RESULTS_DIR=/data/results

RUN apt-get update -qq && \
    apt-get install -y -qq --no-install-recommends \
        git curl ca-certificates jq && \
    rm -rf /var/lib/apt/lists/*

# Bring the prebuilt octobrain binary across — no Rust toolchain needed at runtime.
COPY --from=rust-builder /src/target/release/octobrain /usr/local/bin/octobrain

WORKDIR /bench
COPY benches/requirements.txt /bench/requirements.txt
RUN pip install -r requirements.txt

COPY benches /bench

RUN mkdir -p /data/octobrain /data/bench /data/results

# Default entrypoint runs the LongMemEval pipeline; override with `docker run ... <cmd>`
# to run an alternative target (memora, smoke test, shell).
ENTRYPOINT ["/bench/scripts/entrypoint.sh"]
CMD ["longmemeval"]
