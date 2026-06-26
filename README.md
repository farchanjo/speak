# speak

A single Rust binary: a network client for an OpenAI-compatible speech server
(OmniVoice TTS + faster-whisper ASR at `http://solaris:8800`). It does
text-to-speech, transcription, translation, and a live microphone-translation
loop.

All media is handled **in-process, no subprocess**:

- **Codecs** (decode + resample) use linked `libav*` via
  [`ffmpeg-the-third`](https://crates.io/crates/ffmpeg-the-third)
  (`5.0.0+ffmpeg-8.1`, libavcodec 62). Server audio is decoded through a custom
  in-memory AVIO callback — no temp files, no `ffmpeg` exec.
- **Device I/O + mixing** use the **native macOS CoreAudio mixer** via
  [`objc2-avf-audio`](https://crates.io/crates/objc2-avf-audio): an
  `AVAudioEngine` graph `AVAudioPlayerNode -> mainMixerNode -> outputNode` for
  playback, and an `inputNode` tap for capture. No `afplay`/`ffplay`/`cpal`.

Native playback/capture are macOS-only today; on other platforms the
file-oriented commands still work.

## Build

Requires macOS arm64, Rust 1.95+, Homebrew `ffmpeg` (8.1) and `llvm` (libclang):

```bash
export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig:$PKG_CONFIG_PATH
export LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib
cargo build --release
```

The binary is `target/release/speak` (also symlinked at `bin/speak`).

## Testing

The crate is a thin binary over a reusable library core (`src/lib.rs`), so the
hexagonal modules are exercised both by inline unit tests and by integration
suites under `tests/`.

```bash
export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig:$PKG_CONFIG_PATH
export LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib

cargo test                       # unit + CLI suites (hermetic; no network)
cargo test --features integration  # also runs the live-server suite
```

What is covered:

- **Domain value objects** — voice-design tag validation (accepts
  `"Female, British Accent"`, rejects free text), gen-param keys (`steps`
  alias, `num_steps` rejection), and the retry/backoff policy (geometric
  growth, jitter bounds, `retry_on` classification).
- **Config precedence** — the `flag > env > toml > default` engine and the
  per-key origin recorded for `config show`.
- **Adapters** — the OpenAI `_byot` speech-request body shape (instruct +
  pass-through gen-params), reply interpretation, the daemon's length-prefixed
  framing round-trip over a real `UnixStream` pair, libav WAV muxing / RMS, and
  path/acceleration resolution.
- **CLI** — `--version`/`--help`, `ValueEnum` rejection, completions, the
  voice-design catalog, and `config show` origin reporting, all driven against
  the compiled binary with no network.

The `integration` feature gates a suite that talks to the live server
(`SPEAK_HOST`, default `http://solaris:8800`); each test probes the port first
and **skips with a note** when the server is unreachable, so it is safe to run
anywhere. It is off by default, keeping `cargo test` hermetic.

### CI

A CI job should export the two build env vars above (Homebrew `ffmpeg`/`llvm`)
and run the standard gate; all four commands must exit 0:

```bash
cargo build --release
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo test
```

`~/bin/speckit validate && ~/bin/speckit verify && ~/bin/speckit analyze` gates
the spec corpus. The executable-Gherkin scenarios in
`docs/arch/specs/features/` describe `speak` behavior in prose; speckit's
`verify` grammar binds only `speckit` commands, so those scenarios report as
`unbound` (advisory per ADR-0020) and the gate passes on zero failures.

## Usage

```bash
speak say "Olá, mundo"                 # synthesize (pt-BR) and play
speak say --native "oi"                # use the server's native /tts
speak say --no-play -o out.mp3 "hi"    # save without playing
speak tts "olá" --speed 1.1            # `tts` is an alias of `say`
echo "texto via stdin" | speak say     # stdin fallback

# voice design (canonical tags) + pass-through generation params
speak say --instruct "Female, British Accent" --set num_step=32 "hello"
speak say --list-designs               # list valid voice-design tags

# saved voices (cloning)
speak voices list
speak voices add myvoice --audio ref.wav --ref-text "reference transcript"
speak voices rm myvoice
speak say --voice myvoice "fala com a minha voz"   # clone mode

speak transcribe audio.wav             # -> transcript text
speak transcribe a.mp3 --format json   # extract .text from JSON
speak translate foreign.mp3            # -> English text

speak realtime --from en --to pt-BR --speak   # live mic translation, Ctrl-C to stop

speak check                            # OS + local hw-accel probe + log path
speak health                           # pretty-print /health JSON
speak config init|path|show            # manage the config file
speak completions zsh                  # shell completion script
speak --help                           # full help; --version for the version
```

## Configuration

Precedence: **CLI flags > environment (`SPEAK_*`) > TOML > built-in defaults.**

Global flags: `--host --api-key --lang --voice -q/--quiet`
(`say` additionally takes `--format mp3|opus|aac|flac|wav|pcm`).

Environment: `SPEAK_HOST SPEAK_API_KEY SPEAK_LANG SPEAK_VOICE SPEAK_FORMAT`.

### Acceleration and logging (env)

- `SPEAK_HWACCEL=auto|off|<decoder>` — local libav acceleration. `auto`
  (default) uses all CPU cores (frame threading) and, on macOS, the matching
  AudioToolbox `*_at` decoder. Audio has no GPU/NVENC path (that hardware is
  the server's). Run `speak check` to see what is available.
- `SPEAK_LOG=<level|off>` — rotating file logs under `~/.speak/logs`
  (`info` default; `off` disables). `SPEAK_LOG_DIR` overrides the directory.
  Logs rotate daily with a capped retention (`SPEAK_LOG_RETENTION`, default 7).

TOML at `~/.config/speak/config.toml` (honours `XDG_CONFIG_HOME`):

```toml
host = "http://solaris:8800"
# api_key = "sk-..."
lang = "pt-BR"
voice = "alloy"
format = "mp3"
tts_model = "tts-1"
asr_model = "whisper-1"
# translate_url = "http://solaris:8800/v1/chat/completions"
# translate_model = "gpt-4o-mini"
```

Defaults: host `http://solaris:8800`, lang `pt-BR`, voice `alloy`, format
`mp3`, tts_model `tts-1`, asr_model `whisper-1`. The bearer
`Authorization` header is sent only when an API key is configured.

## How it works

```
                 server bytes (mp3/opus/aac/flac/wav/pcm)
                          |
   reqwest  -------->  libav decode (custom in-memory AVIO)
                          |
                  libswresample  -- 48kHz stereo f32 -->  AVAudioPCMBuffer
                                                              |
                                          AVAudioPlayerNode --+--> mainMixerNode --> outputNode
                                                                        (native OS mixer)

   mic --> AVAudioEngine.inputNode tap --> libav resample 16kHz mono --> in-memory WAV --> Whisper
```
