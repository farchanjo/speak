---
status: accepted
date: 2026-06-26
deciders: [farchanjo]
consulted: []
informed: []
---

# Stay on Rust edition 2021 (defer the 2024 migration)

## Context and Problem Statement

The project's Rust playbook standard targets `edition = "2024"` / `resolver =
"3"` on toolchain 1.95 (edition 2024 is stable since 1.85, the declared MSRV).
`Cargo.toml` and the plan currently declare `edition = "2021"`. The divergence
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
"3"`, and verify with `cargo msrv verify` (MSRV stays 1.85) and a green
`cargo build --release` + `cargo clippy --all-targets -- -D warnings`.

### Consequences

- Good: the build stays green with zero churn on code that is about to be
  replaced; the 2021-vs-2024 divergence is now explicit and bounded.
- Good: the migration has a precise owner (T037) and a verification recipe.
- Bad: the repo temporarily trails the toolchain-2024 convention; the deferral
  must be honoured during the config rebuild rather than slipping further.
