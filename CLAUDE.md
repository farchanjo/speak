# speak — project guide (CLAUDE.md)

`speak` is a single-binary Rust CLI: an **OpenAI-compatible speech client** for the
OmniVoice server (TTS + Whisper ASR + Qwen MT) at `http://solaris:8800` (RTX 4090).
TTS, voice design & cloning, STT, translation, realtime SSE translation, a persistent
daemon, multi-output digital audio routing, and a full layered config.

---

## 0. SPEC-FIRST — non-negotiable

**Every change starts from the spec, never from code.** Workflow for any new feature or
change:

1. `~/bin/speckit specify "<feature>"` → write/refine `docs/arch/sdd/<NNN>/spec.md`.
2. `~/bin/speckit plan` → `plan.md` (which hexagonal layers/modules).
3. `~/bin/speckit tasks` → `tasks.md` (layer-tagged, checkable).
4. Record every real decision as a MADR **ADR** under `docs/arch/adr/` (constitution
   changes require an `accepted` ADR + a `deciders:` entry).
5. Only then implement; keep `tasks.md` checkboxes + ADRs in sync; **commit docs + code
   together**.
6. `~/bin/speckit validate && ~/bin/speckit verify && ~/bin/speckit analyze` must all exit 0.

Do **not** write code without a spec. Keep `docs/arch/` the single source of truth.
The companion server spec lives in a separate speckit project at `~/dev/omnivoice-server`.

---

## 1. Architecture — Hexagonal (Ports & Adapters) + DDD + GoF

Dependencies point **inward** (`adapters → application → domain`). Framework crates appear
**only** under `src/adapters/` (and `clap` under `src/cli/`).

```
src/domain/        pure value objects, zero IO/framework (Voice, VoiceDesign[23 tags],
                   SpeechSpec, GenParams, PcmBuffer, Language, RetryPolicy, DomainError)
src/ports/         trait interfaces (Synthesizer, Transcriber, Translator, AudioSink,
                   AudioSource, AudioDecoder/Encoder, ConfigProvider, VoiceRepository,
                   RealtimeStream, ServerProbe, RetryPolicy, Presenter)
src/application/   use cases + Facade (say/transcribe/translate/realtime/record/voices/check)
src/adapters/      openai (async-openai + extended speech) · coreaudio (AVAudioEngine
                   mixer + multi-output + mic + devices) · libav (ffmpeg-the-third FFI
                   decode/FLAC) · sse (RealtimeStream) · chatmt (Translator) ·
                   presenter (console|json) · retry (backoff+jitter) · daemon · config · http
src/cli/           clap driving adapter (no business logic) → calls the Facade
src/main.rs        composition root (Factory/DI); wires adapters into use cases
```

GoF in use (see ADR-0003): **Adapter** (whole adapters layer), **Strategy** (translate
modes; resampler; retry policy), **Factory** (composition root), **Builder** (requests/
config), **Facade**, **Repository** (voices). Output goes through the **Presenter** port
(stdout, honors `--quiet/--json/--color`); diagnostics go through `tracing` (rotating
`~/.speak/logs` file + stderr when `-v`). **Zero media process-exec** (libav is linked
FFI, CoreAudio is native — never shell out to ffmpeg/afplay/ffplay).

---

## 2. Prerequisites

- **Rust 1.95** toolchain (pinned in `rust-toolchain.toml`), edition 2024, resolver 3.
- **ffmpeg 8.1** with libav* dev libs (Homebrew): provides `libavcodec 62` etc. for the
  `ffmpeg-the-third` FFI.  `brew install ffmpeg`.
- **libclang** (bindgen) — `/opt/homebrew/opt/llvm/lib`.
- macOS (arm64): native CoreAudio via `objc2-avf-audio` (gated `cfg(target_os="macos")`;
  non-macOS uses a clear-error stub).
- A reachable OmniVoice server (default `http://solaris:8800`).

---

## 3. Build

Always export the FFI build env first:

```bash
export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig:$PKG_CONFIG_PATH
export LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib

cargo build --release        # -> target/release/speak  (lto, strip, codegen-units=1)
```

`bin/speak` is a symlink to `target/release/speak`.

### Gates (must all pass before any commit)

```bash
cargo build --release
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo nextest run                       # 259 hermetic tests
cargo nextest run --features integration   # + live tests vs $SPEAK_HOST (skips if unreachable)
~/bin/speckit validate && ~/bin/speckit verify && ~/bin/speckit analyze
```

> Note: `speckit verify` reports the Gherkin scenarios as `unbound` **by design** — its
> ADR-0020 harness can only re-invoke the `speckit` binary, not `speak`. Executable
> acceptance lives in `tests/cli.rs` + `tests/integration.rs`; the trace is in
> `docs/arch/specs/acceptance-coverage.md`.

---

## 4. Release

```bash
export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig:$PKG_CONFIG_PATH LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib
cargo build --release --target aarch64-apple-darwin      # native mac arm64
# (Linux musl targets are installed but need a cross libav toolchain; mac is the primary target.)

V=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
tar -C target/aarch64-apple-darwin/release -czf "speak-${V}-aarch64-apple-darwin.tar.gz" speak
shasum -a 256 "speak-${V}-aarch64-apple-darwin.tar.gz" > "speak-${V}-aarch64-apple-darwin.tar.gz.sha256"
```

Bump `version` in `Cargo.toml` (also surfaced by `speak --version` / `propagate_version`),
tag `vX.Y.Z`, attach the tarball + checksum. Binary is already `strip`ped + `lto`.

---

## 5. Run

```bash
speak say "Olá mundo"                              # TTS, plays via native CoreAudio
speak say --instruct "Female, Young Adult, British Accent" -o out.mp3   # voice design
speak say --voice <saved-name> "..."              # voice clone
speak transcribe audio.mp3                         # STT
speak translate audio.mp3 --format srt             # translate (+ srt/vtt subtitles)
speak realtime --translate --to fr --instruct "Female, British Accent"   # live SSE translation
speak voices add <name> --audio ref.wav [--ref-text "..."]   # voices list | rm
speak devices [--json]                             # list audio in/out devices
speak record -o clip.wav --format wav|flac
speak daemon | daemon status | daemon stop | daemon restart
speak config init | path | show                    # config show prints value + origin
speak completions zsh|bash|fish|powershell
speak check | health | --version
```

Global flags: `--host --api-key --lang --voice --format -q/--quiet --json -v/--verbose
--output-device <id|name>` (repeatable → multi-output fan-out).

---

## 6. Daemon (persistent connection)

`speak daemon` holds one warm pooled connection and listens on a Unix socket
(`~/.speak/speak.sock`); other `speak` invocations forward through it (one-shot fallback
when absent). It is **single-instance**: it writes `~/.speak/speak.pid` atomically;
re-running `speak daemon` **kills the previous instance** (SIGTERM → grace → SIGKILL) and
takes over; clean shutdown removes pidfile + socket. A **health watchdog** probes the
upstream `/health` with a timeout and, after N failures, self-recovers (rebuilds the pool
+ re-probes capabilities, re-discovers the SSE endpoint). See ADR-0010.

---

## 7. Configuration — precedence: flag > ENV (`SPEAK_*`) > `~/.speak/config.toml` > default

Every tunable has a `SPEAK_*` env override and a code default (no magic numbers).
`speak config init` writes a fully-commented `~/.speak/config.toml`; `config show` prints
each value **and its origin**. Sections: `[server]` (host/api_key/timeouts/pool/keepalive/
http2), `[tts]`+`[tts.gen]` (voice/format/speed/instruct + gen-params), `[asr]`,
`[audio.output]` (device/volume→mixer/sample_rate/buffer), `[audio.input]` (mic), `[ffmpeg]`
(threads/resampler/dither/log), `[realtime]` (from/to/translate/chunk), `[daemon]`
(pidfile/socket/kill_grace_ms/health_interval/health_timeout/health_fails/idle/autostart),
`[http]` (translate_url/translate_model/save_dir), `[general]` (quiet/json/color/log/temp_dir).

Key env: `SPEAK_HOST`, `SPEAK_API_KEY`, `SPEAK_LANG`(pt-BR), `SPEAK_VOICE`, `SPEAK_FORMAT`,
`SPEAK_RETRY_*` (max/backoff/jitter/retry_on), `SPEAK_HEALTH_TIMEOUT`,
`SPEAK_DAEMON_HEALTH_INTERVAL`, `SPEAK_TRANSLATE_URL`/`SPEAK_TRANSLATE_MODEL`,
`SPEAK_LOG`/`SPEAK_LOG_DIR`, `SPEAK_CONFIG`.

---

## 8. Server dependency (`solaris:8800`)

The OmniVoice FastAPI server (separate, spec'd in `~/dev/omnivoice-server`) provides:
`/health`, `/v1/models`, `/v1/audio/speech` (+instruct voice-design/clone/gen-params/seed),
`/tts`, `/v1/audio/transcriptions|translations`, `/voices` CRUD, `/v1/chat/completions`
(Qwen2.5-14B MT), and `POST /v1/realtime/translate` (**SSE**: chunk → ASR → MT → TTS →
`transcript|translation|audio|done`). If the server's `server.py` is rewritten and the SSE
route disappears, restore it with **`bash /root/omnivoice/ensure_sse.sh`** (idempotent).

---

## 9. Conventions

- Methods < 30 lines; no dead code; no duplication. Clippy `-D warnings` clean.
- Angular Conventional commits; **small contextual commits**; docs + code committed
  together. Don't push unless asked.
- en-US for all code/docs/commits.

---

## 10. Debugger-grounded analysis (headless lldb)

**When runtime behaviour, a value, a type, or a call path is uncertain — stop the
process and read the truth instead of guessing from source.** A wrong field name or
an assumed value is exactly where an LLM hallucinates; lldb refuses the wrong name
and prints the real one. Drive it non-interactively via the harness in
`scripts/debug/` (everything wrapped in a hard `timeout`; the Rust pretty-printer
banner is stripped).

```bash
# backtrace + real values at a line (grounds "what is cfg here?")
make debug-bt LOC='--file main.rs --line 111' ARGS='config path' P='p cfg->server.host'
#   -> cfg->server.host = "http://solaris:8800"   cfg->server.timeout = 300

# run a command, catch a panic, dump backtrace + locals at the panic site
make debug-panic ARGS='say hi'

# all-thread state of the LIVE daemon, read-only (why is it hung?) — never kills it
make debug-attach            # reads ~/.speak/speak.pid   (or PID=1234)

# optimized build that keeps symbols, to debug release-shaped behaviour
make build-dbg               # -> target/release-dbg/speak
```

Direct scripts (same flags, outside make): `scripts/debug/rust-lldb-batch.sh`,
`rust-panic-trace.sh`, `rust-lldb-attach.sh`.

**Rules:** read SPECIFIC members (`p cfg->server.host`), never `frame variable
*struct` — the whole-struct synthetic walk can stall (the harness caps it with a
timeout anyway). For the forking/self-replacing daemon, `debug-attach` by PID, don't
`launch`. FFI/ObjC frames show as C — set breakpoints on Rust symbols.

**Java** (the Maven/Gradle projects, not this repo): start the JVM with JDWP
(`mvnDebug`, `mvn -Dmaven.surefire.debug test`, or `gradle … --debug-jvm`) then drive
`jdb` headless via `~/bin/jdwp-trace -p 5005 -s 'pkg.Class:42' -c where -c locals -c cont`.
For a live snapshot without breakpoints: `jstack <pid>` / `jcmd <pid> Thread.print`.
