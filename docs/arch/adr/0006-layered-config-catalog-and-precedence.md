---
status: accepted
date: 2026-06-26
deciders: [farchanjo]
consulted: []
informed: []
---

# Layered configuration catalog and precedence

## Context and Problem Statement

`speak` exposes a large surface of tunable settings spanning the HTTP server
connection, TTS, ASR, audio output and input, libav, the realtime pipeline,
the daemon, and general behavior. Users need to set durable defaults once (host,
API key) and override any value per call, with a single, predictable rule for
which source wins, and a way to see where each effective value came from.

## Decision Drivers

- One precedence rule for every key, applied uniformly.
- Durable defaults from a file; per-shell overrides from the environment;
  per-call overrides from flags.
- A `config show` that prints every effective value and its origin so behavior
  is explainable.
- A `config init` that writes a fully commented template.

## Considered Options

- Option A — Four-tier precedence `CLI flag > ENV (SPEAK_*) > ~/.speak/config.toml
  > code default`, a single typed config aggregate assembled by a Builder, and a
  `ConfigProvider` port; `config show` reports the origin (`flag|env|toml|default`)
  per key.
- Option B — Flags and a file only, no environment layer.
- Option C — Environment variables only (twelve-factor), no file.

## Decision Outcome

Chosen option: "Option A". The environment is the highest *persistent* source
(below per-call flags); the file holds durable defaults; code defaults are the
floor. The file lives at `~/.speak/config.toml`, migrating from
`~/.config/speak` if present.

### Catalog (sections and keys)

- `[server]` — `host` (`SPEAK_HOST`, default `http://solaris:8800`), `api_key`
  (`SPEAK_API_KEY`), `timeout_secs`, `connect_timeout_secs`,
  `pool_max_idle_per_host`, `pool_idle_timeout_secs`, `tcp_keepalive_secs`,
  `http2`, `user_agent`.
- `[tts]` — `language` (`pt-BR`), `voice` (`alloy`), `format` (`mp3`), `model`
  (`tts-1`), `speed`, `instruct`, `native`.
- `[tts.gen]` — every generation parameter (default unset): `num_step`,
  `guidance_scale`, `t_shift`, `layer_penalty_factor`, `position_temperature`,
  `class_temperature`, `denoise`, `preprocess_prompt`, `postprocess_output`,
  `audio_chunk_duration`, `audio_chunk_threshold`.
- `[asr]` — `model` (`whisper-1`), `language` (`auto`), `format` (`text`).
- `[audio.output]` — `device`, `volume` (drives `mainMixerNode.outputVolume`),
  `rate`, `sample_rate`, `channels`, `buffer_frames`, `play`.
- `[audio.input]` — `device`, `sample_rate` (`16000`), `channels` (`1`),
  `chunk_secs` (`5`), `silence_threshold_db` (`-40`), `vad`.
- `[ffmpeg]` — `threads`, `resampler`, `resample_quality`, `dither`,
  `sample_fmt`, `log_level`, `extra_filters`.
- `[realtime]` — `from`, `to`, `speak`, `chunk_secs`, `translate`
  (`SPEAK_RT_TRANSLATE`).
- `[daemon]` — `socket` (`~/.speak/speak.sock`), `idle_timeout`, `autostart`.
- `[general]` — `quiet`, `json`, `color`, `temp_dir`, `log`, `config_path`;
  plus `translate_url`, `translate_model`, retry/backoff, `save_dir`.

### Consequences

- Good: one rule, total coverage, and a `config show` that explains origin;
  `config init` emits a commented file; the `ConfigProvider` port keeps the
  rest of the system unaware of TOML/env mechanics.
- Bad: a large catalog to keep in sync across the schema, the template, and
  `config show`; the cross-consistency validator and CUE schema must mirror it.
