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

## Usage

```bash
speak say "Olá, mundo"                 # synthesize (pt-BR) and play
speak say --native "oi"                # use the server's native /tts
speak say --no-play -o out.mp3 "hi"    # save without playing
speak tts "olá" --speed 1.1            # `tts` is an alias of `say`
echo "texto via stdin" | speak say     # stdin fallback

speak transcribe audio.wav             # -> transcript text
speak transcribe a.mp3 --format json   # extract .text from JSON
speak translate foreign.mp3            # -> English text

speak realtime --from en --to pt-BR --speak   # live mic translation, Ctrl-C to stop

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
