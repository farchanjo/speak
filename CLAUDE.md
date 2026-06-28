# speak — project guide (CLAUDE.md)

Single-binary Rust CLI: an **OpenAI-compatible speech client** for the OmniVoice server
(TTS + Whisper ASR + Qwen MT) at `http://solaris:8800` (RTX 4090). Does TTS, voice
design & cloning, STT, translation, realtime SSE translation, decoupled streaming capture,
a persistent daemon, multi-output digital audio routing, and layered config.

**Orientation for an agent working here:**
- Canonical entry point is the **Makefile** — `make help` lists every target. `make`
  targets export the FFI build env automatically; raw `cargo` does not (see §3).
- **Spec-first is non-negotiable** (§0). No code without a spec + ADR.
- When unsure about runtime behaviour, **don't guess — debug it** (§7).
- macOS arm64 only for native audio; `~/.speak/` holds config, logs, pidfile, socket.
- `make gates` (build-release + clippy + fmt + test + spec) must be green before any commit.

---

## 0. SPEC-FIRST — non-negotiable

Every change starts from the spec, never from code:

1. `~/bin/speckit specify "<feature>"` → `docs/arch/sdd/<NNN>/spec.md`
2. `~/bin/speckit plan` → `plan.md` (which hexagonal layers/modules)
3. `~/bin/speckit tasks` → `tasks.md` (layer-tagged, checkable)
4. Record each real decision as a MADR **ADR** in `docs/arch/adr/` (constitution changes
   need an `accepted` ADR + a `deciders:` entry)
5. Implement; keep `tasks.md` + ADRs in sync; **commit docs + code together**
6. `make spec` (= `speckit validate verify analyze`) must exit 0

`docs/arch/` is the single source of truth. Companion server spec lives in a separate
speckit project at `~/dev/omnivoice-server`.

**ADR inventory** (`docs/arch/adr/`, all `accepted` unless noted):

| ID | Title |
|----|-------|
| 0001 | speak — native-media speech client for the solaris server |
| 0002 | Local hardware acceleration probe and rotating logs |
| 0003 | Hexagonal architecture with DDD and named GoF patterns |
| 0004 | async-openai (`_byot`) client and SSE realtime stream |
| 0005 | Persistent daemon over a Unix domain socket (+ autostart refinement) |
| 0006 | Layered configuration catalog and precedence |
| 0007 | Digital multi-output audio routing (fan-out) |
| 0008 | Stay on Rust edition 2021 — **superseded** (now edition 2024 / resolver 3 / MSRV 1.95) |
| 0009 | Output presenter port and tracing diagnostics |
| 0010 | Daemon single-instance lock, health watchdog, forwarded-translate routing |
| 0011 | Realtime/record input-device binding and VAD controls |
| 0012 | Exhaustive CLI short flags |
| 0013 | Single input-channel selection for multi-channel capture devices |
| 0014 | Streaming transcribe over the realtime SSE endpoint (transcript-only) |
| 0015 | Capture source selection and native macOS output tap |
| 0016 | TCC responsibility-disclaim re-exec for host-output capture |
| 0017 | Decoupled streaming-capture pipeline (continuous producer + bounded queue) |
| 0018 | Pipelined in-flight SSE consumer for streaming capture |

> SDD features: `001-…-solaris-server` (accepted), `002-streaming-transcribe-and-capture-source-selection` (draft).
> ADR-0020 is *cited* by `docs/arch/specs/acceptance-coverage.md` for the speckit Gherkin
> harness but the file does not exist yet.

---

## 1. Architecture — Hexagonal (Ports & Adapters) + DDD + GoF

Dependencies point **inward** (`adapters → application → domain`). Ports reference only
`crate::domain` value objects + the `Config` POD — no `reqwest`/`ffmpeg`/`objc2`/`async-openai`
type appears in any port signature. Framework crates live **only** under `src/adapters/`
(and `clap` under `src/cli/`).

```
src/domain/      pure value objects, zero IO/framework: Voice (+VoiceMode/StandardVoice/
                 VoiceClone), VoiceDesign[23 tags], SpeechSpec, GenParams, PcmBuffer
                 (+SampleFormat), Language, AudioFormat, RealtimeMode, RetryPolicy,
                 CaptureSource (+CaptureDirection), DomainError
src/ports/       trait interfaces: Synthesizer (SynthesizedAudio), Transcriber
                 (TranscribeRequest), Translator, AudioSink, AudioSource, AudioDevice
                 (+AudioDeviceId), AudioDecoder/Encoder (RecordFormat), ConfigProvider,
                 VoiceRepository, RealtimeStream (RealtimeFrame), ServerProbe, RetryPolicy,
                 Presenter (Report/Table)
src/application/ use cases + Facade: say · transcribe · translate · realtime ·
                 stream_transcribe (transcribe/translate --stream) · record · voices ·
                 check; internal helpers capture (encode_chunk) + playback (PlaybackStats);
                 fakes.rs test doubles (#[cfg(test)])
src/adapters/    openai (async-openai + extended speech/transcription/translation/voices/probe)
                 · coreaudio (macos: device/engine/stream/tap/disclaim; stub off-mac)
                 · libav (ffmpeg-the-third FFI decode/encode/accel) · sse (SseRealtimeClient)
                 · chatmt (Translator/Qwen MT) · presenter (console|json) · retry
                 (classify/decorator/stream) · daemon (lifecycle/watchdog) · config
                 · http · genparams · headless (HeadlessAudio) · inproc (InProcessSpeech)
src/cli/         clap driving adapter (no business logic) → calls the Facade
src/main.rs      composition root (Factory<'a>, tagged T054); async fn dispatch at main.rs:145
                 routes commands; pre_dispatch_disclaim at main.rs:51 (ADR-0016, pre-logging)
```

**Facade**: `SpeakFacade<Speech, Audio, Codec>` (`application/facade.rs`), generic over three
adapter roles. CLI binds `AppFacade = SpeakFacade<SpeechRole, CoreAudio, LibavCodec>`
(`cli/mod.rs:43`); `SpeechRole` is `Direct(Box<InProcessSpeech>)` (no daemon) or
`Daemon(DaemonSpeechAdapter)` (forwarded).

GoF (ADR-0003): **Adapter** (every `adapters/*`), **Strategy** (`SpeechRole` Direct/Daemon,
`build_presenter` console/json, translate route by target language, retry/resampler),
**Factory** (`Factory<'a>` in `main.rs`, T054), **Decorator** (`retry/decorator.rs` wraps an
adapter), **Builder** (`SpeechSpec::builder`, config), **Facade** (`SpeakFacade`),
**Repository** (`VoiceRepository`). Output → **Presenter** port (`build_presenter` at
`main.rs:211`, honors `--quiet/--json` + `general.color`); diagnostics → `tracing` (rotating
`~/.speak/logs` + stderr on `-v`). **Zero media process-exec** (`main.rs:12`: "Nothing is
shelled out"): libav is linked FFI, CoreAudio is native — never shell out to
ffmpeg/afplay/ffplay. The only sanctioned `current_exe`/`posix_spawn` self-execs are the
autostart daemon (ADR-0005, §5) and the TCC disclaim re-exec (ADR-0016, §4).

---

## 2. Prerequisites

| Need | Detail |
|------|--------|
| Rust 1.95 | pinned in `rust-toolchain.toml` (`channel = "1.95"`, components rustfmt+clippy); edition 2024; resolver 3; MSRV `rust-version = "1.95"` |
| ffmpeg 8.1 + libav* dev | `brew install ffmpeg` → `libavcodec 62` for `ffmpeg-the-third 5.0` FFI |
| libclang (bindgen) | `/opt/homebrew/opt/llvm/lib` (LLVM 22.x) |
| macOS arm64 | native CoreAudio via `objc2-avf-audio` (`cfg(target_os="macos")`; non-mac = error stub) |
| OmniVoice server | reachable, default `http://solaris:8800` |

---

## 3. Build / test / release — use the Makefile (`make help` for all)

`make` targets export the FFI env (`PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig:…`,
`LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib`) themselves. Default `TARGET=aarch64-apple-darwin`.

| Task | Command |
|------|---------|
| Debug build | `make build` (`cargo build`) |
| Release build | `make build-release` → `target/release/speak` (lto, strip, codegen-units=1); `bin/speak` symlinks it via `make link` |
| Fast type-check | `make check` (`cargo check --all-targets`, no codegen) |
| Install | `make install` (`build-release` + `sign` + `link`) — macOS-guarded, no-ops off-mac. With a real identity the installed `bin/speak` is a TCC subject, so `--source output` works (ADR-0015/0016). |
| Codesign | `make sign` — signs `$SIGN_BIN` (default `target/release/speak`); with a **real identity** also applies `$ENTITLEMENTS` (`packaging/macos/speak.entitlements`, `com.apple.security.device.audio-input`) for the host-output tap. Auto-detects the first keychain identity, ad-hoc (`-`) fallback (no entitlement). Override: `CODESIGN_IDENTITY="… (TEAMID)" CODESIGN_OPTS="--options runtime --timestamp"` |
| App bundle | `make app` → `make build` (debug) then `scripts/macos-bundle.sh target/debug/speak target/speak.app` → signed `target/speak.app` (embedded Info.plist + entitlement) for the audio-capture grant (ADR-0016) |
| Lint | `make lint` (`clippy` + `fmt`) · `make fmt-fix` to apply · `make clippy-fix` to auto-apply suggestions |
| Lint (verbose) | `make clippy` (`--all-targets --all-features`) — `clippy::all`+rustc groups **deny**, `pedantic`/`nursery`/`cargo` **warn** (`Cargo.toml [lints]`; tokio-noisy lints allowed there — but `await_holding_lock` is NOT allowed) |
| Lint (strict) | `make clippy-strict` — promotes every warn to a hard error (`-D warnings`; cleanup sessions only) |
| Test | `make test` (`cargo nextest run`, **278 hermetic**) · `make test-int` (`--features integration`, **285** total; live vs `$SPEAK_HOST`, skips if down) |
| Watch / expand / doc | `make watch` (bacon) · `make expand ITEM=…` · `make doc` |
| Spec gates | `make spec` (`validate` + `verify` + `analyze`) |
| **Full pre-commit bar** | `make gates` (build-release + clippy + fmt + test + spec) — green before any commit |
| Release artifact | `make release` → Apple-signs the darwin binary, then `dist/speak-<ver>-<target>.tar.gz` + `.sha256` (`TARGET=` to override) · `make release-all` loops installed targets |
| Cleanup | `make clean` · `dist-clean` · `clean-runtime` (stops daemon, removes sock/pid/logs, keeps config.toml) · `clean-all` |

Raw `cargo` (only if not using make) needs the env first:
```bash
export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig:$PKG_CONFIG_PATH
export LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib
```
`make release` bumps nothing — set `version` in `Cargo.toml` first (also drives
`speak --version`, currently `0.1.0`), then tag `vX.Y.Z`. Mac arm64 is the primary target;
Linux musl targets are installed but need a cross libav toolchain.

**Tag ⟹ Release (MANDATORY — never a tag without a GitHub release).** Every `vX.Y.Z`
git tag MUST ship a matching GitHub release carrying the built artifacts. The exact flow:
```bash
# 1. set version in Cargo.toml (vX.Y.Z without the leading v), commit it
make release                              # → dist/speak-<ver>-<target>.tar.gz + .sha256 (Apple-signed)
git tag -a vX.Y.Z -m "speak vX.Y.Z"       # annotated tag on the release commit
git push origin vX.Y.Z                     # push the tag
gh release create vX.Y.Z \                 # ALWAYS create the release in the same step
  --title "vX.Y.Z" --notes "<highlights>" \
  dist/speak-X.Y.Z-aarch64-apple-darwin.tar.gz \
  dist/speak-X.Y.Z-aarch64-apple-darwin.tar.gz.sha256
```
A pushed tag with no release is a defect — if you tag, you release in the same turn.

> `speckit verify` exits 0; it reports Gherkin scenarios as `unbound` **by design** — its
> harness can only re-invoke `speckit`, never `speak` (so any `I run "speak …"` step is
> unbound: `0 passed / 0 failed / 31 unbound` + 8 from the second feature). Executable
> acceptance lives in `tests/cli.rs` (offline, hermetic) + `tests/integration.rs`
> (server-gated, `--features integration`); trace in `docs/arch/specs/acceptance-coverage.md`.

---

## 4. Run

```bash
speak say "Olá mundo"                                   # TTS → native CoreAudio playback
speak say --instruct "Female, Young Adult, British Accent" -o out.mp3   # voice design (23 tags)
speak say --voice <saved-name> "..."                    # voice clone (-C/--voice global flag)
speak say --list-designs                               # -g: print valid voice-design tags
speak transcribe audio.mp3                              # STT (file, one-shot; FILE optional)
speak transcribe --stream                              # live mic → incremental transcript (ADR-0014)
speak transcribe --stream --source output              # transcribe the PC output (native tap)
speak translate audio.mp3 --format srt                  # translate (file, + srt/vtt)
speak translate --stream --to es                        # live mic → incremental translation (ADR-0017)
speak translate --stream --source output --to fr        # translate the PC output live
speak realtime --translate --to fr --instruct "Female, British Accent"   # live SSE translation
speak realtime -d <id> --no-vad --echo                  # pin input device (ADR-0011), gate off, echo test
speak voices add <name> --audio ref.wav [--ref-text "..."]   # -a/--audio; voices list | rm
speak devices [--json]                                  # audio in/out devices
speak record -o clip.wav -d 8 -D <id> --format wav|flac # -o + -d/--duration REQUIRED; -D = capture device
speak daemon | daemon status | daemon stop | daemon restart
speak config init | path | show                         # `show` prints value + origin
speak completions zsh|bash|fish|powershell
speak check | health | --version
```

**Global flags** (every subcommand): `-H/--host` (`SPEAK_HOST`), `-K/--api-key`,
`-L/--lang`, `-C/--voice` (`-v`/`-V` are taken by verbose/version), `-q/--quiet`,
`-J/--json`, `-v/--verbose` (repeatable: `-v` info / `-vv` debug / `-vvv` trace). There is
**no `--color` global flag** — color is a config key (`[general].color`) only. Every option
has a short (ADR-0012).

**Per-command flags worth knowing:**
- `say`: `-o/--out` · `-n/--no-play` · `-s/--speed` · `-f/--format <mp3|opus|aac|flac|wav|pcm>`
  · `-i/--instruct` · `-r/--ref-text` · `-d/--duration` · `-S/--set KEY=VALUE` (gen params,
  repeatable) · `-D/--output-device <id>` (repeatable → fan-out, FR-11/ADR-0007) ·
  `-g/--list-designs` · `-N/--native` (use `/tts` instead of `/v1/audio/speech`).
- `realtime`: mode group (mutually exclusive, default **Translate**) `-T/--translate` /
  `-n/--no-translate` / `-e/--echo`; `-f/--from` `-t/--to` (default `en`) `-i/--instruct`
  `-D/--output-device` (repeatable). Capture flags shared with streaming (below).
- `record`: `-o/--output` and `-d/--duration` are **required**; `-D/--device <id>`
  `-f/--format <wav|flac>` `-r/--sample-rate` `-c/--channels` `-I/--input-channel` `-s/--source`.
- `transcribe`/`translate`: FILE is **optional** (`Option<PathBuf>` — omit in `--stream`);
  `-f/--format <text|json|srt|vtt|verbose_json>`; `translate` adds `-t/--to` (default `en`).

**Shared capture flags** (`transcribe --stream`, `translate --stream`, `realtime`, `record`):
`-d/--device <id>` pins the input `AudioDeviceID` (ADR-0011 — rebinds the HAL default input);
`-x/--no-vad` + `-F/--vad-floor <dBFS>` (accepts negatives) control the silence gate;
`-I/--input-channel <n>` (also `[audio.input].channel`) captures one 0-based channel of a
multichannel interface (ADR-0013, e.g. SSL 12); `-c/--chunk <secs>` (default 5);
`-s/--source input|output` (ADR-0015, overrides `[audio.capture].source`) selects the capture side.

### Streaming capture pipeline (ADR-0017 — decoupled producer/consumer)

`transcribe --stream`, `translate --stream`, and `realtime` share one continuous pipeline
(the `record` one-shot path is unchanged):

```
native capture (tap IOProc / AVAudioEngine tap)
  └─ RT callback appends interleaved f32 → CaptureRing (VecDeque<f32> behind Mutex,
       drops oldest past cap_secs)
        └─ speak-capture thread (producer): drains chunk_secs every POLL_MS=20 ms,
             blocking_send → bounded tokio mpsc (CHANNEL_CHUNKS=2 slots, ADR-0018)
                  └─ pipelined consumer (cli/stream_pipeline.rs, ADR-0018): up to
                       MAX_INFLIGHT=3 chunk POSTs overlap; encode+VAD per chunk, then
                       collect_chunk POSTs SSE; a FuturesOrdered presents completed
                       chunks in CAPTURE ORDER (never interleaved), racing one pinned
                       Ctrl-C. Throughput = MAX_INFLIGHT / round_trip (vs 1/round_trip)
```

- API: `CoreAudio::capture_stream(source, chunk_secs, cap_secs) -> NativeCaptureStream`
  (`coreaudio/mod.rs`); `NativeCaptureStream::recv() -> Option<PcmBuffer>` wraps the mpsc
  receiver. Tokio stays inside the adapter. Dropping the receiver closes the channel →
  producer exits → native capture RAII teardown.
- Backpressure: bounded mpsc pushes back on the producer → ring grows → drops oldest past
  `cap_secs` (a `tracing` warning fires). Ceiling: `[audio.capture].buffer_secs` (default
  **60.0**, env `SPEAK_AUDIO_CAPTURE_BUFFER_SECS`).
- `transcribe --stream` (ADR-0014): POST `/v1/realtime/translate` with `translate=false`;
  prints only `transcript` frames; `audio`/`translation` frames ignored (no re-voicing/playback).
- `translate --stream` (ADR-0017 extension): same pipeline, POST with `translate=true` +
  `to`; prints only `translation` frames. Both drive the shared pipelined consumer
  `cli::stream_pipeline::run` (ADR-0018): a `build` closure encodes each chunk (VAD-gated)
  and returns the `collect_chunk` future, which POSTs the SSE stream via the shared
  `StreamTranscribeUseCase` (`facade.stream_transcribe_drive`) and collects the wanted
  `FrameKind` (Transcript vs Translation). `cli::stream_options(...)` builds the options.
  Up to `MAX_INFLIGHT` POSTs overlap; a `FuturesOrdered` keeps output in capture order.
- **Ctrl-C**: ONE persistent `let mut shutdown = pin!(tokio::signal::ctrl_c())` outside the
  loop; `select!` races `&mut shutdown` against `drive_one(...)`. WHY: a fresh `ctrl_c()`
  per iteration registers a handler that hasn't subscribed yet → a SIGINT delivered mid-POST
  is missed and the disclaim supervisor's `waitpid` hangs forever. One pinned future
  subscribes immediately and catches SIGINT at any point.

### `--source output` — native tap + permission (ADR-0015/0016)

`output` uses a **native macOS Core Audio tap** (macOS 14.4+): `CATapDescription`
`initStereoGlobalTapButExcludeProcesses` (whole system mix) → `AudioHardwareCreateProcessTap`
→ private aggregate device → read **directly by its `AudioObjectID`** via
`AudioDeviceCreateIOProcID` + `AudioDeviceStart` (`coreaudio/macos/tap.rs`). NOT AVAudioEngine
— the default-input path binds to the real input device (e.g. the SSL interface), not the tap
aggregate, so no audio flows. Full RAII teardown: IoProc → AggregateDevice → ProcessTap.

**Permission** (`kTCCServiceAudioCapture`): macOS attributes the TCC request to the
*responsible process* (the launching terminal), not the signed binary → without disclaim the
tap runs but returns silence. `pre_dispatch_disclaim` (`main.rs:51`, runs **before** logging +
async runtime) checks `cli::wants_output_capture(command, cfg)` for `transcribe --stream` /
`translate --stream` / `record` / `realtime`; if so, `reexec_disclaimed()`
(`coreaudio/macos/disclaim.rs`) `posix_spawn`s `current_exe` with
`responsibility_spawnattrs_setdisclaim(attr,1)` + `POSIX_SPAWN_SETSIGDEF` (SIGINT/TERM/HUP/QUIT
reset so Ctrl-C still stops the child) + `SPEAK_TCC_DISCLAIMED=1` sentinel. The supervisor
parent ignores SIGINT/SIGTERM, `waitpid`s, exits the child status. `speak` becomes its own TCC
subject (`ltd.eonf.speak`) regardless of launching terminal.

One-time setup: (1) `make app` → signed `target/speak.app` (embedded
`NSAudioCaptureUsageDescription` + audio-input entitlement, Apple-Development identity);
(2) grant once via LaunchServices — `open target/speak.app --args record -s output -o
/tmp/x.wav -d 3` → **Allow** (persists by team id). Thereafter direct-exec works from any
terminal: `target/speak.app/Contents/MacOS/speak transcribe --stream --source output`
(verified: direct-exec captured a tone at mean −27.6 dBFS / peak −8.5 dBFS). Run the **bundle**
binary, not `target/debug/speak` — the grant keys on the signed identity. Ad-hoc signing
disclaims to an identity with no grant → silence (a `tracing` warn names `make app`).
**No-permission fallback**: route output to a virtual-loopback device (**BlackHole**) and
capture it as an input (`--source input -d <blackhole-id>`).

---

## 5. Daemon (persistent connection) — ADR-0005 / ADR-0010

`speak daemon` holds one warm pooled connection on a Unix socket (`~/.speak/speak.sock`);
other invocations forward through it (transparent one-shot in-process fallback when absent —
identical result, higher first-byte latency). IPC is **length-prefixed framing** (JSON
header + binary audio payload), NOT a raw HTTP proxy. The warm Facade is
`DaemonFacade = SpeakFacade<InProcessSpeech, HeadlessAudio, LibavCodec>` (`daemon.rs:61`):
`InProcessSpeech` is the retry-wrapped openai + optional chat-MT, target-routed — so forwarded
non-English `translate`/`realtime` honours `--to` (ADR-0010 fix). Local audio (playback,
capture) is **never forwarded** — `say` synthesizes on the daemon (`play=false`) and the CLI
plays the bytes locally; `record`/`realtime` capture stay in the foreground CLI.

**Single-instance** (`daemon/lifecycle.rs`): writes `~/.speak/speak.pid` atomically (temp
sibling + rename). It does **not** fork — `speak daemon` runs `replace_previous()`: if the
predecessor PID is alive AND answering the socket it SIGTERMs → waits `kill_grace_ms`
(default 3000) → SIGKILL, removes sock+pid, then binds the socket in the same process. PID
alive but silent on the socket → treated as PID reuse, left untouched. Clean exit (SIGINT or
SIGTERM) removes both pidfile and socket. `daemon stop` SIGTERMs the pidfile PID (orphan
fallback via socket `Stop` op); `daemon restart` calls `start()` directly (which already
replaces any previous instance — no separate stop step). The `--foreground`/`-f` flag exists
on `DaemonArgs` but is currently unused (`_foreground`) — `speak daemon` always runs foreground
(background it with `&`).

**Autostart** (`[daemon].autostart=true`, `SPEAK_DAEMON_AUTOSTART`, default false; ADR-0005
refinement 2026-06-27): on the first one-shot call with no daemon answering, `daemon::autostart()`
→ `spawn_detached()` resolves `current_exe()`, spawns `<exe> daemon` with `setsid` + null
stdio, polls the socket every 120 ms up to 25× (~3 s), then forwards. On timeout it logs and
falls back to in-process (`Ok(false)`) — never breaks the command. Warm forwarded `say` ≈ 0.4 s
vs ≈ 2.5 s cold (first autostart call). The autostart self-spawn is one of the two sanctioned
`current_exe` execs in the zero-media-exec gate (the other: ADR-0016 disclaim re-exec).

**Health watchdog** (`daemon/watchdog.rs`): probes upstream `/health` via the `ServerProbe`
port every `[daemon].health_interval` s (default 15, **0 disables**), per-probe timeout
`SPEAK_HEALTH_TIMEOUT` (default 5). State machine `Healthy → Degraded → Recovering → Healthy`:
first failure → `Degraded`; the N-th consecutive failure (≥ `health_fails`, default 3, clamped
≥1) → `Recovering` and triggers self-recovery **once** (rebuilds the warm `InProcessSpeech`
Facade + re-runs the realtime capability probe / SSE-endpoint rediscovery, hot-swapped via
`Mutex<Arc<DaemonFacade>>` with the guard NOT held across `.await`). `daemon status` surfaces
`pid`, `uptime_secs`, `requests`, `socket`, `pidfile`, `host`, `health`, `health_failures`,
`health_last_ok_secs`, `health_last_error`, `recoveries`.

> Debugging: attach by PID (`make debug-attach`), never `launch` — `launch` re-runs the binary
> and triggers `replace_previous()` against the live daemon.

---

## 6. Configuration — precedence: flag > ENV (`SPEAK_*`) > `~/.speak/config.toml` > default

Every tunable has a `SPEAK_*` env override and a code default (no magic numbers). Empty-string
env values are ignored (fall through to toml). `SPEAK_CONFIG` overrides the file path; the
legacy `~/.config/speak/config.toml` is read as a one-time fallback. `speak config init` writes
the embedded fully-commented template at `~/.speak/config.toml`; `config show` prints each value
**and its origin** (`flag`/`env`/`toml`/`default`); `api_key` is masked as `***`.

Sections (`adapters/config.rs`):
- `[server]` host(`http://solaris:8800`)/api_key/timeout(300)/connect_timeout(10)/pool_max_idle(8)/
  pool_idle_timeout(90)/tcp_keepalive(60)/http2(false)/user_agent(`speak/<ver>`)
- `[tts]` language(`pt-BR`)/voice(`alloy`)/format(`mp3`)/model(`tts-1`)/speed(1.0)/instruct/native(false)
- `[tts.gen]` 11 optional gen params (num_step, guidance_scale, t_shift, denoise, …) — all unset by default
- `[asr]` model(`whisper-1`)/language/format(`json`)
- `[audio.output]` device/volume(1.0 → mixer)/sample_rate/channels/buffer_frames/play(true)
- `[audio.input]` device(0)/sample_rate(16000)/channels(1)/channel(unset = downmix)/chunk_secs(5.0)/
  silence_threshold_db(−38.0)/vad(true)
- `[audio.capture]` (ADR-0015) source(`input`)/device(unset = system mix)/channel(unset = downmix)/
  buffer_secs(60.0)
- `[ffmpeg]` threads(0 = all)/resampler(`swr`)/resample_quality/dither(true)/sample_fmt/log_level(`error`)/extra_filters
- `[realtime]` from/to(`en`)/**speak**(false — synthesize result)/chunk_secs(5.0)
- `[retry]` (ADR-0004) max_retries(3)/backoff_initial_ms(200)/backoff_max_ms(5000)/multiplier(2.0)/
  jitter(true)/jitter_seed/retry_on(`connect+timeout+5xx+429`)
- `[daemon]` socket/pidfile/idle_timeout(0)/autostart(false)/kill_grace_ms(3000)/health_interval(15)/
  health_timeout(5)/health_fails(3)
- `[http]` (ADR-0006, migrated from legacy `[general]`; `[http]` wins if both present)
  translate_url/translate_model/save_dir
- `[general]` quiet(false)/json(false)/color(true)/temp_dir/log

Key env: `SPEAK_HOST`, `SPEAK_API_KEY`, `SPEAK_LANG`, `SPEAK_VOICE`, `SPEAK_FORMAT`,
`SPEAK_RETRY_MAX` / `SPEAK_RETRY_BACKOFF_MS` / `SPEAK_RETRY_BACKOFF_MAX_MS` /
`SPEAK_RETRY_MULTIPLIER` / `SPEAK_RETRY_JITTER` / `SPEAK_RETRY_ON`,
`SPEAK_HEALTH_TIMEOUT` (no `DAEMON` infix), `SPEAK_DAEMON_HEALTH_INTERVAL`,
`SPEAK_AUDIO_CAPTURE_SOURCE` / `SPEAK_AUDIO_CAPTURE_BUFFER_SECS`,
`SPEAK_TRANSLATE_URL` / `SPEAK_TRANSLATE_MODEL`, `SPEAK_LOG`, `SPEAK_CONFIG`.

---

## 7. Debugger-grounded analysis (headless lldb)

**When a value, type, field, or call path is uncertain — stop the process and read the
truth; do not guess from source.** lldb rejects a wrong field name and prints the real one
— that is the anti-hallucination loop. Verified live: `cfg->server.host =
"http://solaris:8800"`, `cfg->server.timeout = 300`.

```bash
make debug         ARGS='config path'                    # interactive rust-lldb session
make debug-bt LOC='--file main.rs --line 145' ARGS='config path' P='p cfg->server.host'
make debug-panic ARGS='say hi'        # run, catch panic, dump backtrace + locals at the site
make debug-attach                     # all-thread state of the LIVE daemon, read-only (PID= to override)
make build-dbg                        # optimized build WITH symbols → target/release-dbg/speak
```

> `async fn dispatch` is defined at **`main.rs:145`** and runs for every command (the call
> site `dispatch(cli, &cfg).await` is at `main.rs:76`). Safe-to-`run` commands under batch:
> `config path`, `check`, `completions`. Do NOT `run` blocking commands (`say`, `record`,
> streaming) under batch unless the breakpoint is before the blocking call.

> **Full guide: [`scripts/debug/CLAUDE.md`](scripts/debug/CLAUDE.md)** — breakpoints,
> variable analysis, memory dumps, watchpoints, live attach, panics, and the full
> **anti-lock-in doctrine**. Direct scripts: `scripts/debug/rust-lldb-batch.sh` (60 s timeout),
> `scripts/debug/rust-panic-trace.sh`, `scripts/debug/rust-lldb-attach.sh` (30 s, `detach` on exit).

**Anti-lock-in (never wait forever):** bound every session with a `timeout` (the harness
does); verify the breakpoint resolved to **≥1 location** before `run` (`no locations
(pending)` = it will never hit); break on a path the ARGS actually execute. A firing timeout
is data — the process was stuck before the breakpoint → re-run as attach + `bt all`.

---

## 8. Server dependency (`solaris:8800`)

OmniVoice FastAPI server (spec'd separately in `~/dev/omnivoice-server`; default host constant
`DEFAULT_HOST` at `adapters/config.rs:18`) provides the endpoints `speak` calls:

| Endpoint | Adapter |
|----------|---------|
| `GET /health`, `GET /v1/models`, `GET /v1/realtime/translate` (probe) | `openai/probe.rs` |
| `POST /v1/audio/speech` (+instruct voice-design/clone/gen-params/seed), `POST /tts` (legacy) | `openai/speech.rs` |
| `POST /v1/audio/transcriptions` / `…/translations` | `openai/transcription.rs` / `translation.rs` |
| `POST /voices` (multipart) · `GET /voices` · `DELETE /voices/{name}` | `openai/voices.rs` |
| `POST /v1/chat/completions` (Qwen MT) | `chatmt/mod.rs` |
| `POST /v1/realtime/translate` (**SSE**: chunk → ASR → MT → TTS) | `sse/mod.rs` |

SSE frame types (`sse/frame.rs`): `transcript` · `translation` · `audio` · `done` · `error`
(five — `error` is handled in code too). If `server.py` is rewritten and the SSE route
disappears, restore with **`bash /root/omnivoice/ensure_sse.sh`** (idempotent, on the remote).

---

## 9. Conventions

- Methods < 30 lines; no dead code; no duplication; `make clippy` clean (no deny-level hits).
- Angular Conventional commits; **small contextual commits**; docs + code committed together.
  Don't push unless asked.
- **Every git tag MUST have a matching GitHub release** with the built artifacts attached —
  a tag without a release is forbidden. Follow the Tag ⟹ Release flow in §3.
- en-US for all code/docs/commits.
- Never modify `rust-toolchain.toml`, `Cargo.toml [lints.*]`, or PMD/ruleset files without
  explicit permission — fix the code to comply instead.

---

## 10. Gotchas (verified — don't relearn them)

| Trap | Truth |
|------|-------|
| `cfg.field` in lldb expr errors | `cfg` is `&Config` → use `cfg->field` (`p`/`expr`) |
| `frame variable *struct` stalls | Rust synthetic provider can hang on whole structs → read SPECIFIC members (`p cfg->server.host`), or `image lookup -t <Type>` for the field list |
| Panic breakpoint never hits | lowering varies: `panic!`-fmt→`core::panicking::panic_fmt`; `assert!`→`core::panicking::panic` / `assert_failed`; `begin_panic`/`rust_panic` under `std::panicking` (v0-mangled, pending). Use `--func-regex` over the panic families (already in `rust-panic-trace.sh`) |
| `dispatch` breakpoint line | `async fn dispatch` is at **`main.rs:145`** (call site at `main.rs:76`) — NOT 111 |
| Debugging the daemon | no fork; `speak daemon` runs `replace_previous()` (SIGTERMs predecessor) → `make debug-attach` by PID, never `launch` |
| FFI/ObjC frames are opaque | ffmpeg/CoreAudio frames show as C/asm → set breakpoints on Rust symbols only |
| Output-capture returns silence | TCC grant missing or running `target/debug/speak` instead of the signed `speak.app` bundle → `make app` + one-time LaunchServices grant (ADR-0016) |
| `--source output` "no audio" | the tap must be read by its aggregate `AudioObjectID`, not via AVAudioEngine default-input (binds the real input device) |
| Streaming Ctrl-C hangs | a fresh `ctrl_c()` per loop iteration misses mid-POST SIGINT → pin ONE `ctrl_c()` future outside the loop, race it in `select!` |
| `[realtime]` key is `speak` | not `translate` — it's the synthesize-result bool; `[daemon]` key is `idle_timeout`, not `idle` |
| Headless debug hangs | unreachable/pending breakpoint + bare `run` = forever → always `timeout` + check `breakpoint list -b` ≥1 location first |
| `make` vs raw `cargo` | `make` exports the FFI env; raw `cargo` needs the `export` block in §3 |

