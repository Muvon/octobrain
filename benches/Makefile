# Octobrain benchmark targets. All work happens in Docker — no host install.
# Usage from this directory (benches/):
#   cp .env.example .env && edit  # set OPENAI_BASE_URL, OPENAI_API_KEY, models
#   make build                      # build the bench image once
#   make longmemeval                # run the full LongMemEval pipeline
#   make smoke                      # 5-question smoke test against your config
#   make shell                      # open shell in the bench container
#   make clean                      # remove containers + caches (keeps datasets)

COMPOSE := docker compose -f docker/docker-compose.yml

.PHONY: help build longmemeval memora smoke shell clean nuke

help:
	@grep -E '^[a-zA-Z_-]+:.*?##' Makefile | sort | awk 'BEGIN {FS = ":.*?##"}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'

build: ## Build the bench Docker image (octobrain + Python adapter)
	$(COMPOSE) build

longmemeval: ## Run the full LongMemEval pipeline end-to-end
	$(COMPOSE) run --rm bench longmemeval

memora: ## Run the Memora forgetting-aware benchmark (placeholder until impl)
	$(COMPOSE) run --rm bench memora

smoke: ## Run LongMemEval on a 5-question slice — checks the wiring works
	$(COMPOSE) run --rm -e MAX_QUESTIONS=5 bench longmemeval

tune: ## 60-question slice for retrieval-knob tuning (samples all 6 categories)
	$(COMPOSE) run --rm -e MAX_QUESTIONS=60 bench longmemeval

resume: ## Re-score an existing hypothesis.jsonl. Usage: make resume RUN=/data/results/longmemeval-<ts>
	@if [ -z "$(RUN)" ]; then \
		echo "Usage: make resume RUN=/data/results/longmemeval-<timestamp>" >&2; \
		echo "Tip: ls results/ to see available runs" >&2; \
		exit 1; \
	fi
	$(COMPOSE) run --rm -e RESUME_FROM="$(RUN)" bench longmemeval

shell: ## Open a shell inside the bench container for debugging
	$(COMPOSE) run --rm --entrypoint /bin/bash bench

clean: ## Remove the bench container and image (keeps datasets + results)
	$(COMPOSE) down --rmi local --remove-orphans

nuke: ## Full reset: also drop datasets and octobrain DB volume
	$(COMPOSE) down --rmi local --volumes --remove-orphans
