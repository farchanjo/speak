---
status: superseded
date: 2026-06-26
deciders: [farchanjo]
consulted: []
informed: []
---

# Stay on Rust edition 2021 (defer the 2024 migration)

> **Superseded 2026-06-26 — deferral resolved.** The migration trigger named in
> this ADR (the `adapters/config` rebuild, T037) has fired and the bump landed:
> `Cargo.toml` is now `edition = "2024"` / `resolver = "3"` /
> `rust-version = "1.95"`, with a pinned `rust-toolchain.toml`
> (`channel = "1.95"`). The reserved-keyword hazard is gone — the `[tts.gen]`
> field was renamed to `gen_params` (serde `rename = "gen"` keeps the on-disk
> TOML key `[tts.gen]`), not left as the raw `r#gen`. Migration was performed
> with `cargo fix --edition`; the resulting edition-2024 `collapsible_if` /
> `unsafe_op_in_unsafe_fn` / `tail_expr_drop_order` lints were resolved with
> let-chains and explicit `unsafe` blocks. Verified GREEN: `cargo build
> --release`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --
> --check`, and the full `cargo test` suite. (`cargo msrv verify` is not
> installed locally; the sole toolchain is rustc/cargo 1.95.0, so a green 1.95
> build is the effective MSRV gate.) Option B below is retained for the
> historical record.

## Context and Problem Statement

The project's required Rust target is `edition = "2024"` / `resolver = "3"` with
`rust-version = "1.95"` and a pinned `rust-toolchain.toml` (`channel = "1.95"`).
Edition 2024 itself is stable since 1.85, but the project pins MSRV 1.95 to match
the sole local/CI toolchain (rustc/cargo 1.95.0). `Cargo.toml` and the plan
currently declare `edition = "2021"` / `rust-version = "1.85"`. The divergence
was unacknowledged, so a reviewer cannot tell whether 2021 is intentional or an
oversight. We must either align to 2024 now or record an explicit, time-bounded
deferral.

## Decision Drivers

- Keep `cargo build --release` GREEN at every commit (the binding project rule).
- Avoid churn on flat modules (`client.rs`, `config.rs`, `audio_macos.rs`, ...)
  that the ADR-0003 hexagonal rebuild is about to replace wholesale.
- Match the toolchain convention (edition 2024) as soon as it is low-risk.

## Considered Options

- Option A — Migrate to edition 2024 / resolver 3 now via `cargo fix --edition`.
- Option B — Stay on edition 2021 with a recorded deferral and a concrete
  migration trigger.

## Decision Outcome

Chosen option: "Option B", staying on edition 2021 for now.

A trial bump to `edition = "2024"` fails to compile: the config layer uses the
identifier `gen` (the `[tts.gen]` struct field and its accessor), which edition
2024 reserves as a keyword, producing ~19 `expected identifier, found reserved
keyword 'gen'` errors plus an unparsable serde-derive expansion. Migrating
correctly means renaming to the raw identifier `r#gen` (or, better, to
`gen_params`) across the config aggregate and its serde mapping — code that the
hexagonal rebuild (tasks T013/T037, `domain::GenParams` + `adapters/config`)
will rewrite anyway. Forcing `r#gen` onto soon-to-be-deleted flat code is
negative-value churn.

Migration trigger: edition 2024 / resolver 3 is adopted as part of the
`adapters/config` rebuild (T037), where the `gen` field becomes the
`domain::GenParams` value object and the raw-identifier hazard disappears. At
that point run `cargo fix --edition`, bump `edition = "2024"` + `resolver =
"3"` + `rust-version = "1.95"`, add a `rust-toolchain.toml` (`channel =
"1.95"`), and verify with `cargo msrv verify` (MSRV 1.95) and a green
`cargo build --release` + `cargo clippy --all-targets -- -D warnings`.

### Consequences

- Good: the build stays green with zero churn on code that is about to be
  replaced; the 2021-vs-2024 divergence is now explicit and bounded.
- Good: the migration has a precise owner (T037) and a verification recipe.
- Bad: the repo temporarily trails the toolchain-2024 convention; the deferral
  must be honoured during the config rebuild rather than slipping further.
