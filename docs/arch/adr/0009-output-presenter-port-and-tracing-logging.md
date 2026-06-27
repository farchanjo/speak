---
status: accepted
date: 2026-06-26
deciders: [farchanjo]
consulted: []
informed: []
---

# Output presenter port and tracing diagnostics

## Context and Problem Statement

`speak` accreted raw `println!`/`eprintln!` calls across the flat modules to
print command output (`check`, `config show`, `--list-designs`, transcripts,
realtime captions). This conflates two distinct streams: the command **RESULT**
(what a caller pipes or parses) and **DIAGNOSTICS** (progress, warnings, errors).
It also makes the global `--quiet`, `--json` (FR-16), and `--color`/`NO_COLOR`
toggles impossible to honour consistently, and leaves output untestable — there
is no seam to capture. Hexagonal layering (ADR-0003) forbids the application use
cases from owning a concrete writer, yet today they would, the moment any
println leaks inward. We need one auditable seam for results and one for
diagnostics, each independently swappable and testable.

## Decision Drivers

- A command RESULT must go to stdout; DIAGNOSTICS must go to stderr and the
  rotating log file — never interleaved on the same stream.
- The `--quiet`, `--json`, and `--color`/`NO_COLOR` behaviour must be honoured in
  exactly one place, not re-derived per call site.
- Use cases and the CLI must emit results through an abstraction, so the domain
  and application layers never bind to a concrete writer (ADR-0003).
- Output must be unit-testable via a capture buffer.
- Diagnostics must already flow through the existing `tracing` stack (ADR-0002).

## Considered Options

- Option A — A `Presenter` driven port (structured `Report`/`Table`/`line`
  primitives) with swappable `console | json | buffer` adapters for RESULTS, and
  `tracing` for all DIAGNOSTICS (stderr + rotating file, gated by a `-v`/`--verbose`
  count and `RUST_LOG`/`SPEAK_LOG`).
- Option B — Keep `println!`/`eprintln!` but funnel them through a couple of
  free helper functions that read a global `--quiet`/`--json` flag.
- Option C — Return fully rendered `String`s from every use case and let the CLI
  print them.

## Decision Outcome

Chosen option: "Option A". A narrow `Presenter` port keeps the application layer
writer-agnostic, makes `--json` a pure adapter swap (not a branch in every use
case), and yields a trivial capture-buffer test double. Option B re-scatters the
`--quiet`/`--json` decision and still has no test seam; Option C couples the use
cases to presentation formatting and cannot express colour/quiet without leaking
flags inward.

### The split

- Command RESULT -> stdout, via the `Presenter` port.
- Diagnostics/logs -> stderr (when verbosity is enabled via `-v`/`--verbose` +
  `RUST_LOG`/`SPEAK_LOG`) and ALWAYS to the rotating `~/.speak/logs` file, via
  `tracing` (ADR-0002). No `println!`/`eprintln!` for either concern.

### The port

`src/ports/presenter.rs` defines `trait Presenter` with three structured
primitives — `report(&Report)` (titled key/value blocks: `check`, `config show`,
`say --json` metadata), `table(&Table)` (`devices`, `voices list`,
`--list-designs`), and `line(&str)` (a transcript, a realtime caption). `Report`
and `Table` are pure presentation value objects assembled through fluent
builders; no framework type crosses the boundary, and JSON serialisation stays
the json adapter's concern (the port carries no `serde_json` type). The trait is
object-safe so the composition root can inject a `Box<dyn Presenter>`.

### Adapters (later stage)

`src/adapters/presenter` provides the swappable implementations: a `console`
adapter that renders coloured, aligned human text and honours `--quiet`
(suppress) and `--color`/`NO_COLOR`; a `json` adapter that renders each payload
as machine-readable JSON for `--json` (FR-16); and a capture-buffer used in
tests. The composition root selects the adapter from the global flags and
injects it; the CLI and the use cases emit through the port, replacing the
existing `println!` piles.

### Named GoF patterns

- Strategy — the `console | json | buffer` presenters are interchangeable
  rendering strategies selected at the composition root.
- Builder — `Report` and `Table` are assembled through fluent builders.

### Consequences

- Good: results are pipeable/parseable and independently testable; `--quiet`/
  `--json`/`--color` live in one adapter; the application layer stays pure.
- Good: diagnostics and results never collide on one stream, and every
  diagnostic is captured in the rotating log regardless of console verbosity.
- Bad: a new port + adapters and the mechanical conversion of the existing
  `println!`/`eprintln!` call sites (tracked in `tasks.md` as the adapter and CLI
  wiring tasks). The port trait + value objects land first (this layer); the
  console/json adapters and the call-site conversion follow.
