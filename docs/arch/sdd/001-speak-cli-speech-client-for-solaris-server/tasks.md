# Tasks: speak

## Task Breakdown

- [x] T001 Cargo project: deps (tokio, reqwest, clap+clap_complete, serde, toml,
  anyhow, ffmpeg-the-third, objc2/objc2-avf-audio/block2); `[profile.release]` lto/strip.
- [x] T002 `config.rs`: `Config`/`FileConfig` + precedence (flags > env > toml > defaults),
  XDG-aware path.
- [x] T003 `client.rs`: `SpeechClient` with `health`, `speak`, `speak_native`, `transcribe`,
  `translate`, `chat_translate`.
- [x] T004 `codec.rs`: libav custom-AVIO decode -> PCM, libswresample resample, in-memory
  WAV mux, RMS silence gate.
- [x] T005 `audio_macos.rs`: native CoreAudio `play` (AVAudioPlayerNode -> mainMixerNode)
  and `capture_chunk` (inputNode tap); `audio_stub.rs` for other OSes.
- [x] T006 `main.rs`: clap CLI + `say|tts`, `transcribe`, `translate`, `health`, `config`,
  `completions`.
- [x] T007 `realtime` command: native capture -> ASR -> translate -> print + optional TTS,
  loop to Ctrl-C.
- [x] T008 `cargo build --release`; smoke-test `health`, `say`, `transcribe`/`translate`
  round-trip against solaris.
- [x] T009 `config init` writes `~/.config/speak/config.toml`; install symlink `bin/speak`.
- [x] T010 README + build env vars; `speckit validate`/`verify`.

## Dependencies

- Reachable server `http://solaris:8800` (running OmniVoice + Whisper).
- macOS arm64 + Homebrew ffmpeg 8.1 (libavcodec 62) + LLVM (libclang). Rust 1.95.
- Build env: `PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig`, `LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib`.
