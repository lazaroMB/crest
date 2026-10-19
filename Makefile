# crest Makefile.
#
# Compatible with GNU make 3.81 (macOS) and 4.x: no .ONESHELL or $(file ...).
# Every documented target carries a "## " comment that `make help` shows.
# Every variable below can be overridden: `make build CARGO=cross`.

CARGO     ?= cargo
BIN       ?= crest
PACKAGE   ?= crest
WORKSPACE ?= .
PREFIX    ?= $(HOME)/.local
SCRIPTS   ?= scripts
BASH      ?= bash

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo "Usage: make <target> [VAR=value]"
	@echo ""
	@awk 'BEGIN { FS = ":.*## " } /^[a-zA-Z0-9_-]+:.*## / { printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@echo ""
	@echo "Variables: CARGO=$(CARGO) WORKSPACE=$(WORKSPACE) PREFIX=$(PREFIX) PROMPT=\"...\""

.PHONY: build
build: ## Build the whole workspace (debug)
	$(CARGO) build --workspace

.PHONY: release
release: ## Build the optimized binary (target/release/crest)
	$(CARGO) build --release -p $(PACKAGE)

.PHONY: fmt
fmt: ## Format all the code (cargo fmt)
	$(CARGO) fmt --all

.PHONY: fmt-check
fmt-check: ## Check formatting without changing files
	$(CARGO) fmt --all -- --check

.PHONY: lint
lint: ## Strict clippy over the whole workspace (-D warnings)
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

# Uses cargo-nextest when installed (faster); nextest does not run
# doctests, so in that case they run separately with `cargo test --doc`.
NEXTEST := $(shell command -v cargo-nextest 2>/dev/null)

.PHONY: test
test: ## Offline tests (uses cargo-nextest when installed)
ifneq ($(NEXTEST),)
	$(CARGO) nextest run --workspace --no-tests=pass
	$(CARGO) test --workspace --doc
else
	$(CARGO) test --workspace
endif

# Tests against the real API are marked #[ignore] and their name contains
# "live"; they also read ANTHROPIC_LIVE_TEST=1 and need ANTHROPIC_API_KEY in
# the environment (never in files).
.PHONY: test-live
test-live: ## Tests against the real API (ignored; needs ANTHROPIC_API_KEY)
	ANTHROPIC_LIVE_TEST=1 $(CARGO) test --workspace -- --ignored live

.PHONY: deps
deps: ## Install the external dependencies (scripts/install-deps.sh)
	$(BASH) $(SCRIPTS)/install-deps.sh

.PHONY: deps-dev
deps-dev: ## Install dependencies and development tools (--dev)
	$(BASH) $(SCRIPTS)/install-deps.sh --dev

.PHONY: doctor
doctor: ## Check the dependencies without installing anything (--check)
	$(BASH) $(SCRIPTS)/install-deps.sh --check

.PHONY: arch
arch: ## Hexagonal architecture rules (scripts/check-arch.sh)
	$(BASH) $(SCRIPTS)/check-arch.sh

.PHONY: test-arch
test-arch: ## Tests for scripts/check-arch.sh
	$(BASH) tests/arch/run.sh

.PHONY: test-deps
test-deps: ## Tests for scripts/install-deps.sh
	$(BASH) tests/install-deps/test-helpers.sh
	$(BASH) tests/install-deps/test-macos-sim.sh

.PHONY: test-install
test-install: ## Tests for scripts/install.sh and the app bundle of scripts/package.sh
	$(BASH) tests/install/run.sh

.PHONY: check
check: fmt-check lint test arch test-arch test-deps test-install ## What must pass to close a phase

.PHONY: ci
ci: doctor check ## doctor + check

.PHONY: run
run: ## Open the app on a directory (WORKSPACE=<dir>, default .)
	$(CARGO) run -p $(PACKAGE) -- $(WORKSPACE)

.PHONY: run-headless
run-headless: ## Run a prompt without the UI (PROMPT="..." required, WORKSPACE=<dir>)
ifeq ($(strip $(PROMPT)),)
	@echo 'error: PROMPT is missing. Usage: make run-headless PROMPT="..." [WORKSPACE=<dir>]' >&2
	@exit 2
else
	$(CARGO) run -p $(PACKAGE) -- --headless "$(PROMPT)" --workspace $(WORKSPACE)
endif

.PHONY: package
package: release ## Build the release tarball (dist/)
	$(BASH) $(SCRIPTS)/package.sh

.PHONY: install
install: release ## Install crest as an app: app menu on Linux, ~/Applications on macOS (PREFIX=~/.local)
	tarball="$$($(BASH) $(SCRIPTS)/package.sh --out target/dist | tail -n 1)" && \
		sh $(SCRIPTS)/install.sh --tarball "$$tarball" --prefix "$(PREFIX)"

.PHONY: uninstall
uninstall: ## Remove what `make install` installed (PREFIX=~/.local)
	sh $(SCRIPTS)/install.sh --uninstall --prefix "$(PREFIX)"

.PHONY: clean
clean: ## Delete the build artifacts (cargo clean)
	$(CARGO) clean
