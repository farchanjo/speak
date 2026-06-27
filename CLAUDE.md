# speak — project guide (CLAUDE.md)

Single-binary Rust CLI: an **OpenAI-compatible speech client** for the OmniVoice server
(TTS + Whisper ASR + Qwen MT) at `http://solaris:8800` (RTX 4090). Does TTS, voice
design & cloning, STT, translation, realtime SSE translation, a persistent daemon,
multi-output digital audio routing, and layered config.

**Orientation for an agent working here:**
- Canonical entry point is the **Makefile** — `make help` lists every target. `make`
  targets export the FFI build env automatically; raw `cargo` does not (see §3).
- **Spec-first is non-negotiable** (§0). No code without a spec + ADR.
- When unsure about runtime behaviour, **don't guess — debug it** (§7).
- macOS arm64 only for native audio; `~/.speak/` holds config, logs, pidfile, socket.

---

## 0. SPEC-FIRST — non-negotiable

Every change starts from the spec, never from code:

1. `~/bin/speckit specify "<feature>"` → `docs/arch/sdd/<NNN>/spec.md`
2. `~/bin/speckit plan` → `plan.md` (which hexagonal layers/modules)
3. `~/bin/speckit tasks` → `tasks.md` (layer-tagged, checkable)
4. Record each real decision as a MADR **ADR** in `docs/arch/adr/` (constitution changes
   need an `accepted` ADR + a `deciders:` entry)
5. Implement; keep `tasks.md` + ADRs in sync; **commit docs + code together**
6. `make spec` (= `speckit validate && verify && analyze`) must exit 0

`docs/arch/` is the single source of truth. Companion server spec lives in a separate
speckit project at `~/dev/omnivoice-server`.

---

## 1. Architecture — Hexagonal (Ports & Adapters) + DDD + GoF

Dependencies point **inward** (`adapters → application → domain`). Framework crates live
**only** under `src/adapters/` (and `clap` under `src/cli/`).

```
src/domain/      pure value objects, zero IO/framework (Voice, VoiceDesign[23 tags],
                 SpeechSpec, GenParams, PcmBuffer, Language, RetryPolicy, DomainError)
src/ports/       trait interfaces (Synthesizer, Transcriber, Translator, AudioSink,
                 AudioSource, AudioDecoder/Encoder, ConfigProvider, VoiceRepository,
                 RealtimeStream, ServerProbe, RetryPolicy, Presenter)
src/application/ use cases + Facade (say/transcribe/translate/realtime/record/voices/check)
src/adapters/    openai (async-openai + extended speech) · coreaudio (AVAudioEngine mixer
                 + multi-output + mic + devices) · libav (ffmpeg-the-third FFI decode/FLAC)
                 · sse (RealtimeStream) · chatmt (Translator) · presenter (console|json)
                 · retry (backoff+jitter) · daemon · config · http
src/cli/         clap driving adapter (no business logic) → calls the Facade
src/main.rs      composition root (Factory/DI); dispatch() at main.rs:111 routes commands
```

GoF (ADR-0003): **Adapter** (adapters layer), **Strategy** (translate modes / resampler /
retry), **Factory** (composition root), **Builder** (requests/config), **Facade**,
**Repository** (voices). Output → **Presenter** port (honors `--quiet/--json/--color`);
diagnostics → `tracing` (rotating `~/.speak/logs` + stderr on `-v`). **Zero media
process-exec**: libav is linked FFI, CoreAudio is native — never shell out to
ffmpeg/afplay/ffplay.

---

## 2. Prerequisites

| Need | Detail |
|------|--------|
| Rust 1.95 | pinned in `rust-toolchain.toml`; edition 2024; resolver 3 |
| ffmpeg 8.1 + libav* dev | `brew install ffmpeg` → `libavcodec 62` for `ffmpeg-the-third` FFI |
| libclang (bindgen) | `/opt/homebrew/opt/llvm/lib` |
| macOS arm64 | native CoreAudio via `objc2-avf-audio` (`cfg(target_os="macos")`; non-mac = error stub) |
| OmniVoice server | reachable, default `http://solaris:8800` |

---

## 3. Build / test / release — use the Makefile (`make help` for all)

`make` targets export the FFI env (`PKG_CONFIG_PATH`, `LIBCLANG_PATH`) themselves.

| Task | Command |
|------|---------|
| Debug build | `make build` |
| Release build | `make build-release` → `target/release/speak` (lto, strip); `bin/speak` symlinks it |
| Lint | `make lint` (verbose clippy + fmt check) · `make fmt-fix` to apply · `make clippy-fix` to auto-apply suggestions |
| Lint (verbose) | `make clippy` — `all`+rustc groups **deny**, `pedantic`/`nursery`/`cargo` **warn** (config in `Cargo.toml [lints]`; tokio-noisy lints allowed there) |
| Lint (strict) | `make clippy-strict` — promotes every warn to a hard error (cleanup sessions only) |
| Test | `make test` (259 hermetic) · `make test-int` (live vs `$SPEAK_HOST`, skips if down) |
| Spec gates | `make spec` (speckit validate/verify/analyze) |
| **Full pre-commit bar** | `make gates` (build-release + clippy + fmt + test + spec) — green before any commit |
| Release artifact | `make release` → `dist/speak-<ver>-<target>.tar.gz` + `.sha256` (`TARGET=` to override) |
| Cleanup | `make clean` · `dist-clean` · `clean-runtime` (keeps config.toml) · `clean-all` |

Raw `cargo` (only if not using make) needs the env first:
```bash
export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig:$PKG_CONFIG_PATH
export LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib
```
`make release` bumps nothing — set `version` in `Cargo.toml` first (also drives
`speak --version`), then tag `vX.Y.Z`. Mac arm64 is the primary target; Linux musl targets
are installed but need a cross libav toolchain.

> `speckit verify` exits 0; it reports Gherkin scenarios as `unbound` **by design** (its
> ADR-0020 harness can only re-invoke `speckit`, not `speak`). Executable acceptance lives
> in `tests/cli.rs` + `tests/integration.rs`; trace in `docs/arch/specs/acceptance-coverage.md`.

---

## 4. Run

```bash
speak say "Olá mundo"                                   # TTS → native CoreAudio playback
speak say --instruct "Female, Young Adult, British Accent" -o out.mp3   # voice design (23 tags)
speak say --voice <saved-name> "..."                    # voice clone
speak transcribe audio.mp3                              # STT
speak translate audio.mp3 --format srt                  # translate (+ srt/vtt)
speak realtime --translate --to fr --instruct "Female, British Accent"   # live SSE translation
speak voices add <name> --audio ref.wav [--ref-text "..."]   # voices list | rm
speak devices [--json]                                  # audio in/out devices
speak record -o clip.wav --format wav|flac
speak daemon | daemon status | daemon stop | daemon restart
speak config init | path | show                         # `show` prints value + origin
speak completions zsh|bash|fish|powershell
speak check | health | --version
```
Global flags: `--host --api-key --lang --voice --format -q/--quiet --json -v/--verbose
--output-device <id|name>` (repeatable → multi-output fan-out).

---

## 5. Daemon (persistent connection) — ADR-0010

`speak daemon` holds one warm pooled connection on a Unix socket (`~/.speak/speak.sock`);
other invocations forward through it (one-shot fallback when absent). **Single-instance**:
writes `~/.speak/speak.pid` atomically; re-running `speak daemon` **kills the previous
instance** (SIGTERM → grace → SIGKILL) and takes over; clean shutdown removes pidfile +
socket. A **health watchdog** probes upstream `/health` and, after N failures, self-recovers
(rebuilds pool, re-probes capabilities, re-discovers the SSE endpoint). It **forks and
self-replaces** → when debugging, attach by PID, never `launch` (§7).

---

## 6. Configuration — precedence: flag > ENV (`SPEAK_*`) > `~/.speak/config.toml` > default

Every tunable has a `SPEAK_*` env override and a code default (no magic numbers).
`speak config init` writes a fully-commented `config.toml`; `config show` prints each value
**and its origin**. Sections: `[server]` (host/api_key/timeouts/pool/keepalive/http2),
`[tts]`+`[tts.gen]` (voice/format/speed/instruct + gen-params), `[asr]`, `[audio.output]`
(device/volume→mixer/sample_rate/buffer), `[audio.input]` (mic), `[ffmpeg]`
(threads/resampler/dither/log), `[realtime]` (from/to/translate/chunk), `[daemon]`
(pidfile/socket/kill_grace_ms/health_interval/health_timeout/health_fails/idle/autostart),
`[http]` (translate_url/translate_model/save_dir), `[general]` (quiet/json/color/log/temp_dir).

Key env: `SPEAK_HOST`, `SPEAK_API_KEY`, `SPEAK_LANG` (pt-BR), `SPEAK_VOICE`, `SPEAK_FORMAT`,
`SPEAK_RETRY_*` (max/backoff/jitter/retry_on), `SPEAK_HEALTH_TIMEOUT`,
`SPEAK_DAEMON_HEALTH_INTERVAL`, `SPEAK_TRANSLATE_URL`/`SPEAK_TRANSLATE_MODEL`,
`SPEAK_LOG`/`SPEAK_LOG_DIR`, `SPEAK_CONFIG`.

---

## 7. Debugger-grounded analysis (headless lldb)

**When a value, type, field, or call path is uncertain — stop the process and read the
truth; do not guess from source.** lldb rejects a wrong field name and prints the real one
— that is the anti-hallucination loop. Verified live: `cfg->server.host =
"http://solaris:8800"`, `cfg->server.timeout = 300`.

```bash
make debug-bt LOC='--file main.rs --line 111' ARGS='config path' P='p cfg->server.host'
make debug-panic ARGS='say hi'        # run, catch panic, dump backtrace + locals at the site
make debug-attach                     # all-thread state of the LIVE daemon, read-only (PID= to override)
make build-dbg                        # optimized build WITH symbols → target/release-dbg/speak
```

> **Full guide: [`scripts/debug/CLAUDE.md`](scripts/debug/CLAUDE.md)** — breakpoints,
> variable analysis, memory dumps, watchpoints, live attach, panics, and the full
> **anti-lock-in doctrine**. Direct scripts: `scripts/debug/rust-lldb-batch.sh`,
> `rust-panic-trace.sh`, `rust-lldb-attach.sh` (all timeout-wrapped).

**Anti-lock-in (never wait forever):** bound every session with a `timeout` (the harness
does); verify the breakpoint resolved to **≥1 location** before `run` (`no locations
(pending)` = it will never hit); break on a path the ARGS actually execute.

---

## 8. Server dependency (`solaris:8800`)

OmniVoice FastAPI server (spec'd separately in `~/dev/omnivoice-server`) provides: `/health`,
`/v1/models`, `/v1/audio/speech` (+instruct voice-design/clone/gen-params/seed), `/tts`,
`/v1/audio/transcriptions|translations`, `/voices` CRUD, `/v1/chat/completions` (Qwen2.5-14B
MT), `POST /v1/realtime/translate` (**SSE**: chunk → ASR → MT → TTS →
`transcript|translation|audio|done`). If `server.py` is rewritten and the SSE route
disappears, restore with **`bash /root/omnivoice/ensure_sse.sh`** (idempotent).

---

## 9. Conventions

- Methods < 30 lines; no dead code; no duplication; `make clippy` clean (no deny-level hits).
- Angular Conventional commits; **small contextual commits**; docs + code committed together.
  Don't push unless asked.
- en-US for all code/docs/commits.

---

## 10. Gotchas (verified this session — don't relearn them)

| Trap | Truth |
|------|-------|
| `cfg.field` in lldb expr errors | `cfg` is `&Config` → use `cfg->field` (`p`/`expr`) |
| `frame variable *struct` stalls | Rust synthetic provider can hang on whole structs → read SPECIFIC members, or `image lookup -t <Type>` for the field list |
| Panic breakpoint never hits | lowering varies: `panic!`-fmt→`core::panicking::panic_fmt`; `assert!`→`core::panicking::panic` / `std::panicking::begin_panic`; `rust_panic` is v0-mangled (pending). Use `--func-regex` over the panic families (already in `rust-panic-trace.sh`) |
| Debugging the daemon | it forks + SIGTERMs its predecessor → `make debug-attach` by PID, never `launch` |
| FFI/ObjC frames are opaque | ffmpeg/CoreAudio frames show as C/asm → set breakpoints on Rust symbols only |
| Headless debug hangs | unreachable/pending breakpoint + bare `run` = forever → always `timeout` + check `breakpoint list -b` ≥1 location first |
| `make` vs raw `cargo` | `make` exports the FFI env; raw `cargo` needs the `export` block in §3 |
