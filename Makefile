# Makefile de crest.
#
# Compatible con GNU make 3.81 (macOS) y 4.x: sin .ONESHELL ni $(file ...).
# Cada objetivo documentado lleva un comentario "## " que `make help` muestra.
# Todas las variables de abajo se pueden sobreescribir: `make build CARGO=cross`.

CARGO     ?= cargo
BIN       ?= crest
PACKAGE   ?= crest
WORKSPACE ?= .
PREFIX    ?= $(HOME)/.local
SCRIPTS   ?= scripts

.DEFAULT_GOAL := help

.PHONY: help
help: ## Muestra esta ayuda
	@echo "Uso: make <objetivo> [VAR=valor]"
	@echo ""
	@awk 'BEGIN { FS = ":.*## " } /^[a-zA-Z0-9_-]+:.*## / { printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@echo ""
	@echo "Variables: CARGO=$(CARGO) WORKSPACE=$(WORKSPACE) PREFIX=$(PREFIX) PROMPT=\"...\""

.PHONY: build
build: ## Compila todo el workspace (debug)
	$(CARGO) build --workspace

.PHONY: release
release: ## Compila el binario optimizado (target/release/crest)
	$(CARGO) build --release -p $(PACKAGE)

.PHONY: fmt
fmt: ## Formatea todo el código (cargo fmt)
	$(CARGO) fmt --all

.PHONY: fmt-check
fmt-check: ## Verifica el formato sin modificar archivos
	$(CARGO) fmt --all -- --check

.PHONY: lint
lint: ## Clippy estricto sobre todo el workspace (-D warnings)
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Usa cargo-nextest si está instalado (más rápido); nextest no corre
# doctests, así que en ese caso se corren aparte con `cargo test --doc`.
NEXTEST := $(shell command -v cargo-nextest 2>/dev/null)

.PHONY: test
test: ## Tests sin red (usa cargo-nextest si está instalado)
ifneq ($(NEXTEST),)
	$(CARGO) nextest run --workspace --no-tests=pass
	$(CARGO) test --workspace --doc
else
	$(CARGO) test --workspace
endif

# Los tests contra la API real están marcados #[ignore] y su nombre contiene
# "live"; además leen ANTHROPIC_LIVE_TEST=1 y necesitan ANTHROPIC_API_KEY en
# el entorno (nunca en archivos).
.PHONY: test-live
test-live: ## Tests contra la API real (ignorados; requiere ANTHROPIC_API_KEY)
	ANTHROPIC_LIVE_TEST=1 $(CARGO) test --workspace -- --ignored live
