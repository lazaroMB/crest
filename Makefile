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
BASH      ?= bash

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

# scripts/install-deps.sh lo escribe T0.3.
.PHONY: deps
deps: ## Instala las dependencias externas (scripts/install-deps.sh)
	$(BASH) $(SCRIPTS)/install-deps.sh

.PHONY: deps-dev
deps-dev: ## Instala dependencias y herramientas de desarrollo (--dev)
	$(BASH) $(SCRIPTS)/install-deps.sh --dev

.PHONY: doctor
doctor: ## Verifica las dependencias sin instalar nada (--check)
	$(BASH) $(SCRIPTS)/install-deps.sh --check

.PHONY: arch
arch: ## Reglas de la arquitectura hexagonal (scripts/check-arch.sh)
	$(BASH) $(SCRIPTS)/check-arch.sh

.PHONY: test-arch
test-arch: ## Tests de scripts/check-arch.sh
	$(BASH) tests/arch/run.sh

.PHONY: test-deps
test-deps: ## Tests de scripts/install-deps.sh
	$(BASH) tests/install-deps/test-helpers.sh
	$(BASH) tests/install-deps/test-macos-sim.sh

.PHONY: check
check: fmt-check lint test arch test-arch test-deps ## Lo que debe pasar para cerrar una fase

.PHONY: ci
ci: doctor check ## doctor + check

.PHONY: run
run: ## Abre la app sobre un directorio (WORKSPACE=<dir>, por defecto .)
	$(CARGO) run -p $(PACKAGE) -- $(WORKSPACE)

.PHONY: run-headless
run-headless: ## Corre un prompt sin UI (PROMPT="..." obligatorio, WORKSPACE=<dir>)
ifeq ($(strip $(PROMPT)),)
	@echo 'error: falta PROMPT. Uso: make run-headless PROMPT="..." [WORKSPACE=<dir>]' >&2
	@exit 2
else
	$(CARGO) run -p $(PACKAGE) -- --headless "$(PROMPT)" --workspace $(WORKSPACE)
endif

.PHONY: install
install: release ## Copia target/release/crest a PREFIX/bin (PREFIX=~/.local)
	mkdir -p "$(PREFIX)/bin"
	cp target/release/$(BIN) "$(PREFIX)/bin/$(BIN)"
	@echo "instalado en $(PREFIX)/bin/$(BIN)"

.PHONY: clean
clean: ## Borra los artefactos de compilación (cargo clean)
	$(CARGO) clean
