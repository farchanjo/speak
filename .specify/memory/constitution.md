# speak Constitution

This document establishes the foundational principles and governance
model for the speak project. It guides every decision about
scope, contribution, and evolution of the system.

## Principles

1. Clarity over cleverness: every design choice must be explainable
   to a newcomer in a single paragraph.
2. Constraints create focus: the project scope is bounded; features
   outside that scope belong in a separate initiative.
3. Reproducibility by default: any automated action must yield the
   same result when run twice on the same input.
4. Hexagonal architecture with DDD and named GoF patterns: the codebase
   is organized as Ports & Adapters. Dependencies point inward only
   (`adapters -> application -> domain`); the domain layer is pure and
   performs zero I/O. Ubiquitous-language domain types (Value Objects,
   Entities, Aggregates) live in `domain`; ports are traits; adapters
   implement ports against concrete technology (async-openai, CoreAudio,
   libav, TOML, Unix socket). Reusable solutions are realized through
   explicitly named Gang of Four patterns — Adapter, Strategy, Factory,
   Builder, Facade, Repository — and each is recorded in the governing
   ADR so the structure is auditable rather than incidental.
5. All media is in-process: decode, resample, playback, and capture use
   linked libraries via FFI. No media operation may spawn a child
   process (`ffmpeg`, `ffplay`, `afplay`, `ffprobe`); the end-to-end
   path is fully digital.

## Governance

Changes to this constitution require a recorded Architecture Decision
(MADR) with status `accepted` and at least one `deciders` entry. Trivial
corrections (typos, formatting) may be committed directly; structural
changes must go through the ADR process.

Principle 4 (Hexagonal + DDD + GoF) is recorded in
`docs/arch/adr/0003-hexagonal-ddd-gof-architecture.md` (status
`accepted`, deciders `[farchanjo]`). Principle 5 (in-process media,
no exec) is recorded in
`docs/arch/adr/0001-speak-cli-speech-client-for-solaris-server.md`.
The full set of accepted ADRs (0001–0008) under `docs/arch/adr/` forms
the binding decision record; any change to a principle above must amend
or supersede the corresponding ADR.
