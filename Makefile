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

# ---- Apple code signing (macOS only; no-ops elsewhere) ------------------------
# `make install` / `make release` sign the Mach-O binary on macOS. The default
# identity is the first valid codesigning identity in the keychain; it falls back
# to ad-hoc ("-") when none exists (e.g. CI), so the build never breaks off-mac.
# Distribution build (Developer ID + hardened runtime, notarization-ready):
#   make install CODESIGN_IDENTITY="Developer ID Application: Name (TEAMID)" \
#                CODESIGN_OPTS="--options runtime --timestamp"
UNAME_S           := $(shell uname -s)
# Leave empty to auto-detect the first keychain identity at sign time (ad-hoc
# fallback). Set explicitly for a chosen cert (e.g. a Developer ID).
CODESIGN_IDENTITY ?=
CODESIGN_OPTS     ?=
SIGN_BIN          ?= $(BIN)
# Audio-input entitlement for the host-output tap (ADR-0015/0016); applied only
# with a real signing identity (TCC ignores ad-hoc signatures).
ENTITLEMENTS      ?= packaging/macos/speak.entitlements

.DEFAULT_GOAL := help
.PHONY: help build build-release build-dbg run install link sign app \
        check clippy clippy-strict clippy-fix fmt fmt-fix test test-int watch expand doc lint gates \
        debug debug-bt debug-panic debug-attach \
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

install: build-release sign link ## Build release + Apple code-sign (macOS) + symlink bin/speak

link: ## Refresh bin/speak symlink -> target/release/speak
	@mkdir -p bin && ln -sf ../$(BIN) bin/speak && echo "bin/speak -> $(BIN)"

sign: ## Apple code-sign $(SIGN_BIN) (macOS only; auto-detects identity, ad-hoc fallback)
	@if [ "$(UNAME_S)" != "Darwin" ]; then \
		echo "⏭  codesign skipped — not macOS (uname=$(UNAME_S))"; \
	elif [ ! -f "$(SIGN_BIN)" ]; then \
		echo "✗ codesign: binary not found: $(SIGN_BIN)" >&2; exit 1; \
	else \
		id="$(CODESIGN_IDENTITY)"; \
		[ -n "$$id" ] || id="$$(security find-identity -v -p codesigning 2>/dev/null | awk '/[0-9]+\)/{print $$2; exit}')"; \
		[ -n "$$id" ] || id="-"; \
		if [ "$$id" = "-" ]; then \
			echo "🔏 codesign [ad-hoc] $(SIGN_BIN)"; \
		else \
			echo "🔏 codesign [$$id] $(SIGN_BIN)"; \
		fi; \
		ent=""; \
			if [ "$$id" != "-" ] && [ -f "$(ENTITLEMENTS)" ]; then \
				ent="--entitlements $(ENTITLEMENTS)"; \
				echo "   + entitlement $(ENTITLEMENTS) (host-output tap)"; \
			fi; \
			codesign --force --sign "$$id" $$ent $(CODESIGN_OPTS) "$(SIGN_BIN)" || exit 1; \
		codesign --verify --strict --verbose=2 "$(SIGN_BIN)" || exit 1; \
		echo "✅ signed + verified"; \
		codesign --display --verbose=2 "$(SIGN_BIN)" 2>&1 \
			| grep -E 'Identifier=|Authority=|TeamIdentifier=|Signature=' || true; \
	fi

## ---------------------------------------------------------------- macos app bundle
app: build ## Signed speak.app so `--source output` can get the audio-capture TCC grant (macOS, ADR-0015)
	@if [ "$(UNAME_S)" != "Darwin" ]; then \
		echo "⏭  app bundle skipped — not macOS (uname=$(UNAME_S))"; \
	else \
		CODESIGN_IDENTITY="$(CODESIGN_IDENTITY)" ./scripts/macos-bundle.sh target/debug/speak target/speak.app; \
	fi

## ---------------------------------------------------------------- debug / quality
check: ## Fast type-check (no codegen)
	$(CARGO) check --all-targets

clippy: ## Verbose lint: all-group+rustc DENY, pedantic/nursery/cargo WARN (see Cargo.toml [lints])
	$(CARGO) clippy --all-targets --all-features

clippy-strict: ## Zero-tolerance: promote every warn (incl. pedantic/nursery) to a hard error
	$(CARGO) clippy --all-targets --all-features -- -D warnings

clippy-fix: ## Auto-apply machine-applicable clippy suggestions to the working tree
	$(CARGO) clippy --fix --all-targets --all-features --allow-dirty --allow-staged

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

## ---------------------------------------------------------------- debug (headless lldb)
# Drive lldb non-interactively to ground analysis on real runtime state instead
# of guessing from source. See scripts/debug/ and CLAUDE.md §10.
build-dbg: ## Optimized build WITH symbols (-> target/release-dbg/speak)
	$(CARGO) build --profile release-dbg

debug: build ## Interactive rust-lldb session: make debug ARGS='config path'
	rust-lldb -- target/debug/speak $(ARGS)

debug-bt: build ## Headless: break at LOC, dump backtrace+exprs. LOC='--file main.rs --line 111' ARGS='config path' [P='p cfg->server.host']
	scripts/debug/rust-lldb-batch.sh -k '$(LOC)' -c 'thread backtrace -c 12' $(if $(P),-c '$(P)',) -- $(ARGS)

debug-panic: build ## Headless: run ARGS, catch panic, dump backtrace+locals. make debug-panic ARGS='say hi'
	scripts/debug/rust-panic-trace.sh -- $(ARGS)

debug-attach: ## Headless: all-thread state of the live daemon (read-only). make debug-attach [PID=1234]
	scripts/debug/rust-lldb-attach.sh $(PID)

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
release: ## Build + Apple-sign (darwin) + tarball + sha256 for $(TARGET) -> dist/
	$(CARGO) build --release --target $(TARGET)
	@case "$(TARGET)" in \
		*apple-darwin*) $(MAKE) --no-print-directory sign SIGN_BIN=target/$(TARGET)/release/speak ;; \
		*) echo "⏭  codesign skipped — non-Apple target $(TARGET)" ;; \
	esac
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
