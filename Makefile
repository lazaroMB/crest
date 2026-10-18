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
