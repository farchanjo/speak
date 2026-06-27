# speak — build / release / debug helper
#
# `make` with no target prints help. Every documented target carries a `## ` note.
# Spec-first project (see CLAUDE.md §0): the `gates` target is the pre-commit bar.

# ---- FFI build environment (ffmpeg-the-third needs pkg-config + libclang) -------
# Exported at the Makefile level so every recipe shell inherits them.
export PKG_CONFIG_PATH := /opt/homebrew/lib/pkgconfig:$(PKG_CONFIG_PATH)
export LIBCLANG_PATH   := /opt/homebrew/opt/llvm/lib

# ---- knobs (override on the CLI: `make release TARGET=x86_64-unknown-linux-musl`)
CARGO   ?= cargo
SPECKIT ?= $(HOME)/bin/speckit
TARGET  ?= aarch64-apple-darwin
VERSION := $(shell grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
BIN     := target/release/speak
DIST    := dist
LOGDIR  := $(HOME)/.speak/logs

.DEFAULT_GOAL := help
.PHONY: help build build-release run install link \
        check clippy fmt fmt-fix test test-int watch expand doc lint gates \
        spec validate verify analyze \
        release release-all \
        daemon daemon-status daemon-stop daemon-restart logs health \
        clean dist-clean clean-runtime clean-all

## ---------------------------------------------------------------- help
help: ## Show this help
	@echo "speak v$(VERSION) — targets:"
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

## ---------------------------------------------------------------- build
build: ## Debug build
	$(CARGO) build

build-release: ## Optimized host build (-> target/release/speak)
	$(CARGO) build --release

run: ## Run debug build (pass args via ARGS=…): make run ARGS='say "hi"'
	$(CARGO) run -- $(ARGS)

install: build-release link ## Build release + symlink bin/speak

link: ## Refresh bin/speak symlink -> target/release/speak
	@mkdir -p bin && ln -sf ../$(BIN) bin/speak && echo "bin/speak -> $(BIN)"

## ---------------------------------------------------------------- debug / quality
check: ## Fast type-check (no codegen)
	$(CARGO) check --all-targets

clippy: ## Lint (deny warnings)
	$(CARGO) clippy --all-targets -- -D warnings

fmt: ## Check formatting
	$(CARGO) fmt --all -- --check

fmt-fix: ## Apply formatting
	$(CARGO) fmt --all

test: ## Hermetic test suite (cli + gates + unit)
	$(CARGO) nextest run

test-int: ## Live tests vs $$SPEAK_HOST (skips if unreachable)
	$(CARGO) nextest run --features integration

watch: ## Bacon watch loop (needs `cargo binstall bacon`)
	bacon

expand: ## Macro-expand (needs cargo-expand): make expand ITEM=cli
	$(CARGO) expand $(ITEM)

doc: ## Build + open API docs (private items)
	$(CARGO) doc --no-deps --document-private-items --open

lint: clippy fmt ## clippy + fmt-check

## ---------------------------------------------------------------- spec gates
validate: ## speckit validate
	$(SPECKIT) validate

verify: ## speckit verify (Gherkin scenarios are `unbound` by design — see CLAUDE.md §3)
	$(SPECKIT) verify

analyze: ## speckit analyze
	$(SPECKIT) analyze

spec: validate verify analyze ## All speckit gates

gates: build-release clippy fmt test spec ## FULL pre-commit bar (build+lint+fmt+test+spec)
	@echo "✅ all gates green"

## ---------------------------------------------------------------- release
release: ## Build + tarball + sha256 for $(TARGET) -> dist/
	$(CARGO) build --release --target $(TARGET)
	@mkdir -p $(DIST)
	tar -C target/$(TARGET)/release -czf $(DIST)/speak-$(VERSION)-$(TARGET).tar.gz speak
	@shasum -a 256 $(DIST)/speak-$(VERSION)-$(TARGET).tar.gz \
		| tee $(DIST)/speak-$(VERSION)-$(TARGET).tar.gz.sha256
	@echo "✅ dist/speak-$(VERSION)-$(TARGET).tar.gz"

release-all: ## Release for every installed rustup target (linux musl needs cross libav)
	@for t in $$(rustup target list --installed); do \
		echo "==> $$t"; $(MAKE) --no-print-directory release TARGET=$$t || echo "  ⚠ $$t failed (cross toolchain?)"; \
	done

## ---------------------------------------------------------------- daemon (runtime debug)
daemon: build-release ## Start the persistent daemon (unix socket + watchdog)
	$(BIN) daemon

daemon-status: ## Daemon status
	@$(BIN) daemon status 2>/dev/null || speak daemon status

daemon-stop: ## Stop the daemon
	@$(BIN) daemon stop 2>/dev/null || speak daemon stop

daemon-restart: ## Restart the daemon
	@$(BIN) daemon restart 2>/dev/null || speak daemon restart

logs: ## Tail the newest rotating log in ~/.speak/logs
	@test -d $(LOGDIR) && tail -f "$$(ls -t $(LOGDIR)/* | head -1)" || echo "no logs in $(LOGDIR)"

health: ## Probe the upstream server health
	@$(BIN) health 2>/dev/null || speak health

## ---------------------------------------------------------------- cleanup
clean: ## cargo clean (build artifacts in target/)
	$(CARGO) clean

dist-clean: ## Remove dist/ release tarballs + checksums
	rm -rf $(DIST)

clean-runtime: ## Remove daemon socket/pid + rotating logs (keeps config.toml)
	-@$(BIN) daemon stop 2>/dev/null || speak daemon stop 2>/dev/null || true
	@rm -f  $(HOME)/.speak/speak.sock $(HOME)/.speak/speak.pid
	@rm -rf $(LOGDIR)
	@echo "🧹 runtime cleaned (config.toml preserved)"

clean-all: clean dist-clean clean-runtime ## Full cleanup: target/ + dist/ + runtime state
	@echo "🧹 all clean"
