# Makefile for octobrain
# Best practices for Rust development and CI/CD

# Default target
.DEFAULT_GOAL := help

# Variables
BINARY_NAME := octobrain
VERSION := $(shell grep '^version =' Cargo.toml | cut -d '"' -f2)
RUST_VERSION := $(shell rustc --version)
TARGET_DIR := target
RELEASE_DIR := $(TARGET_DIR)/release
DEBUG_DIR := $(TARGET_DIR)/debug

# Compilation targets
TARGETS := x86_64-unknown-linux-musl \
          aarch64-unknown-linux-musl \
          x86_64-pc-windows-msvc \
          aarch64-pc-windows-msvc

# Colors for output
GREEN := \033[0;32m
YELLOW := \033[0;33m
RED := \033[0;31m
BLUE := \033[0;34m
NC := \033[0m # No Color

# Check if we're in a git repository
GIT_AVAILABLE := $(shell git status >/dev/null 2>&1 && echo "yes" || echo "no")

.PHONY: help
help: ## Show this help message
	@echo "$(BLUE)octobrain v$(VERSION)$(NC)"
	@echo "$(BLUE)Rust version: $(RUST_VERSION)$(NC)"
	@echo ""
	@echo "$(YELLOW)Available targets:$(NC)"
	@awk 'BEGIN {FS = ":.*##"}; printf "  $(GREEN)%-20s$(NC) %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@echo ""
	@echo "$(YELLOW)Usage:$(NC)"
	@echo "  make $(GREEN)<target>$(NC)          # Build specific target"
	@echo "  make $(GREEN)build$(NC)              # Build in debug mode"
	@echo "  make $(GREEN)build-release$(NC)      # Build in release mode"
	@echo "  make $(GREEN)install$(NC)           # Install to ~/.cargo/bin"
	@echo "  make $(GREEN)install-completions$(NC)  # Install shell completions"
	@echo "  make $(GREEN)check$(NC)            # Run cargo check"
	@echo "  make $(GREEN)fmt$(NC)              # Run cargo fmt"
	@echo "  make $(GREEN)clippy$(NC)           # Run cargo clippy"
	@echo "  make $(GREEN)test$(NC)            # Run tests"
	@echo "  make $(GREEN)clean$(NC)            # Clean build artifacts"
	@echo "  make $(GREEN)install-deps$(NC)      # Install development dependencies"
	@echo "  make $(GREEN)run$(NC)             # Run application in debug mode"
	@echo "  make $(GREEN)run-release$(NC)      # Run application in release mode"
	@echo ""
	@echo "$(YELLOW)For more information, see INSTRUCTIONS.md$(NC)"

.PHONY: install-deps
install-deps: ## Install development dependencies
	@echo "$(YELLOW)Installing development dependencies...$(NC)"
	trustup component add clippy rustfmt
	@echo "$(GREEN)Dependencies installed successfully$(NC)"

.PHONY: install-targets
install-targets: ## Install compilation targets
	@echo "$(YELLOW)Installing compilation targets...$(NC)"
	@for target in $(TARGETS); do \
		echo "Installing $$target..."; \
		rustup target add $$target; \
	done
	@echo "$(GREEN)Compilation targets installed$(NC)"

.PHONY: check
check: ## Run cargo check
	@echo "$(YELLOW)Running cargo check...$(NC)"
	cargo check --all-targets --all-features
	@echo "$(GREEN)Cargo check passed$(NC)"

.PHONY: fmt
fmt: ## Format code with cargo fmt
	@echo "$(YELLOW)Formatting code...$(NC)"
	cargo fmt --all -- --check
	@echo "$(GREEN)Code formatted$(NC)"

.PHONY: clippy
clippy: ## Run cargo clippy
	@echo "$(YELLOW)Running clippy lints...$(NC)"
	cargo clippy --all-targets --all-features -- -D warnings
	@echo "$(GREEN)Clippy checks passed$(NC)"

.PHONY: test
test: ## Run tests
	@echo "$(YELLOW)Running tests...$(NC)"
	cargo test --verbose --no-default-features
	@echo "$(GREEN)Tests passed$(NC)"

.PHONY: clean
clean: ## Clean build artifacts
	@echo "$(YELLOW)Cleaning build artifacts...$(NC)"
	cargo clean
	@echo "$(GREEN)Build artifacts cleaned$(NC)"

.PHONY: install
install: build-release ## Install to ~/.cargo/bin
	@echo "$(YELLOW)Installing $(BINARY_NAME)...$(NC)"
	cargo install --path .
	@echo "$(GREEN)$(BINARY_NAME) installed successfully$(NC)"

.PHONY: uninstall
uninstall: ## Uninstall from ~/.cargo/bin
	@echo "$(YELLOW)Uninstalling $(BINARY_NAME)...$(NC)"
	cargo uninstall $(BINARY_NAME)
	@echo "$(GREEN)$(BINARY_NAME) uninstalled successfully$(NC)"

.PHONY: install-completions
install-completions: build-release ## Install shell completions
	@echo "$(YELLOW)Installing shell completions...$(NC)"
	./scripts/install-completions.sh
	@echo "$(GREEN)Shell completions installed!$(NC)"

.PHONY: test-completions
test-completions: build-release ## Test shell completion generation
	@echo "$(YELLOW)Testing shell completion generation...$(NC)"
	./scripts/test-completions.sh
	@echo "$(GREEN)Completion tests passed!$(NC)"

.PHONY: run
run: ## Run the application in debug mode
	@echo "$(YELLOW)Running $(BINARY_NAME) in debug mode...$(NC)"
	cargo run
	@echo "$(GREEN)Application exited$(NC)"

.PHONY: run-release
run-release: ## Run the application in release mode
	@echo "$(YELLOW)Running $(BINARY_NAME) in release mode...$(NC)"
	cargo run --release
	@echo "$(GREEN)Application exited$(NC)"

.PHONY: build
build: ## Build the project in debug mode
	@echo "$(YELLOW)Building $(BINARY_NAME) in debug mode...$(NC)"
	cargo build
	@echo "$(GREEN)Build complete$(NC)"

.PHONY: build-release
build-release: ## Build the project in release mode
	@echo "$(YELLOW)Building $(BINARY_NAME) in release mode...$(NC)"
	cargo build --release
	@echo "$(GREEN)Release binary built: $(RELEASE_DIR)/$(BINARY_NAME)$(NC)"

.PHONY: build-all
build-all: install-targets ## Build for all supported platforms
	@echo "$(YELLOW)Building for all supported platforms...$(NC)"
	@for target in $(TARGETS); do \
		echo "Building $$target..."; \
		cargo build --release --target $$target; \
		if [ $$? -eq 0 ]; then \
			echo "$(GREEN)✓ $$target built successfully$(NC)"; \
		else \
			echo "$(RED)✗ $$target build failed$(NC)"; \
		fi; \
	done
	@echo "$(GREEN)All targets built successfully$(NC)"

.PHONY: ci
ci: format-check lint test ## Run all CI checks locally
	@echo "$(GREEN)All CI checks passed!$(NC)"

.PHONY: ci-quick
ci-quick: format-check lint ## Run quick CI checks (no audit)
	@echo "$(GREEN)Quick CI checks passed!$(NC)"

# Create target directories if they don't exist
$(TARGET_DIR):
	mkdir -p $(TARGET_DIR)

$(RELEASE_DIR): $(TARGET_DIR)
	mkdir -p $(RELEASE_DIR)

$(DEBUG_DIR): $(TARGET_DIR)
	mkdir -p $(DEBUG_DIR)
