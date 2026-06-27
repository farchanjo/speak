---
status: accepted
date: 2026-06-27
deciders: [farchanjo]
consulted: []
informed: []
---

# Exhaustive CLI short flags

## Context and Problem Statement

Most `speak` options were long-only (`--host`, `--instruct`, `--output-device`, …).
Only a handful had short aliases (`-o`, `-q`, `-v`). For an interactive,
daily-driver CLI the user asked for a short flag on **every** option so common
invocations stay terse.

The constraint is clap's flag namespace: `-h` (help) is reserved on every command,
`-V`/`--version` is reserved at the root, and **global** options (`global = true`)
propagate into every subcommand, so their shorts must not collide with any
subcommand's shorts. Each subcommand's own shorts must also be internally unique.

## Decision Drivers

- One short per long option wherever a non-colliding letter exists.
- Globals get a stable, mnemonic, collision-free set reused across all subcommands.
- Mnemonic first; where the natural letter is taken, pick a clear alternate
  (capitalized variant of the same letter, or a related letter) rather than dropping
  the short.
- The whole surface must pass `Cli::command().debug_assert()` (the existing
  `cli_definition_is_valid` test) — clap rejects any duplicate at construction.

## Considered Options

1. **Leave most options long-only (status quo).** Rejected — the user asked for
   terse daily invocations; long-only is verbose for common flags.
2. **Add shorts only to the most-used options.** Partial; leaves an inconsistent,
   hard-to-predict surface where some flags have a short and most do not.
3. **One short per option, case-disambiguated where the natural letter is taken.**
   Complete and predictable, bounded only by clap's namespace rules (reserved
   `h`/`q`/`v`/`V`, globals propagating into subcommands).

## Decision Outcome

Chosen option: **Option 3 (one short per option)** — a fixed short-flag map.
Globals use capitals to stay clear of the lowercase subcommand letters; reserved
letters (`h`, `q`, `v`, `V`) are never reused.

**Global (propagated to all subcommands):**

| Long | Short |
|------|-------|
| `--host` | `-H` |
| `--api-key` | `-K` |
| `--lang` | `-L` |
| `--voice` | `-C` (clone/voice; `-v`/`-V` are taken) |
| `--json` | `-J` |
| `--quiet` | `-q` (existing) |
| `--verbose` | `-v` (existing) |

**`say`:** `-o`/`--out`, `-n`/`--no-play`, `-s`/`--speed`, `-f`/`--format`,
`-i`/`--instruct`, `-r`/`--ref-text`, `-d`/`--duration`, `-S`/`--set`,
`-D`/`--output-device`, `-g`/`--list-designs`, `-N`/`--native`.

**`transcribe`:** `-l`/`--language`, `-f`/`--format`.

**`translate`:** `-t`/`--to`, `-f`/`--format`.

**`realtime`:** `-f`/`--from`, `-t`/`--to`, `-T`/`--translate`, `-n`/`--no-translate`,
`-e`/`--echo`, `-i`/`--instruct`, `-D`/`--output-device`, `-c`/`--chunk`,
`-d`/`--device`, `-x`/`--no-vad`, `-F`/`--vad-floor`.

**`record`:** `-o`/`--output`, `-d`/`--duration`, `-D`/`--device`, `-f`/`--format`,
`-r`/`--sample-rate`, `-c`/`--channels`.

**`voices add`:** `-a`/`--audio`, `-r`/`--ref-text`.

**`daemon`:** `-f`/`--foreground`.

Positional arguments (`TEXT`, `FILE`, `NAME`, `SHELL`) take no short.
`devices --json` keeps its existing long form and inherits the global `-J`.

### Consequences

- Good: terse daily use (`speak say -i "Female, British Accent" -s 1.1 -o out.mp3`).
- Good: the map is documented and regression-guarded by `debug_assert`.
- Neutral: a few shorts are case-disambiguated (`-s`/`-S`, `-d`/`-D`, `-n`/`-N`,
  `-f`/`-F`, `-c`/`-C`) — standard clap practice; `--help` lists both forms.
- Constraint: `--voice` cannot be `-v`/`-V` (taken by verbose/version), hence `-C`.
