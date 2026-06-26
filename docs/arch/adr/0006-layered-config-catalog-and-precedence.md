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
- `[audio.output]` — `device` (a device **name**, resolved to an
  `AudioDeviceID`), `volume` (drives `mainMixerNode.outputVolume`), `rate`,
  `sample_rate`, `channels`, `buffer_frames`, `play`. `rate` is the playback
  device's nominal hardware sample rate requested from CoreAudio (the output
  node rate); `sample_rate` is the PCM sample rate the libav resampler targets
  before feeding the mixer. They coincide unless the device runs at a rate other
  than the decode target, in which case CoreAudio resamples `sample_rate` ->
  `rate` at the output node.
- `[audio.input]` — `device` (a device **name**, same form as
  `[audio.output].device` — never a numeric index), `sample_rate` (`16000`),
  `channels` (`1`), `chunk_secs` (`5`), `silence_threshold_db` (`-40`), `vad`.
- `[ffmpeg]` — `threads`, `resampler`, `resample_quality`, `dither`,
  `sample_fmt`, `log_level`, `extra_filters`.
- `[realtime]` — `from`, `to`, `speak`, `chunk_secs`, `translate`
  (`SPEAK_RT_TRANSLATE`). `translate` and `speak` are **distinct** keys, not
  aliases: `translate` selects translate-vs-passthrough mode (the `--translate`
  / `--no-translate` flag, default from `SPEAK_RT_TRANSLATE`), while `speak`
  toggles whether the produced text is spoken back through TTS.
- `[daemon]` — `socket` (`~/.speak/speak.sock`), `idle_timeout`, `autostart`.
- `[http]` — the non-OpenAI chat-MT endpoint and save directory:
  `translate_url` (`SPEAK_TRANSLATE_URL`), `translate_model`
  (`SPEAK_TRANSLATE_MODEL`), `save_dir` (`SPEAK_SAVE_DIR`). `translate_url`
  enables an arbitrary `--to` target in the realtime pipeline (FR-8); without
  it the client degrades to the source transcript.
- `[retry]` — the configurable exponential-backoff + jitter resilience policy
  (FR-17, ADR-0004) wrapping **every** network call: `max_retries`
  (`SPEAK_RETRY_MAX`, default `3`), `backoff_initial_ms`
  (`SPEAK_RETRY_BACKOFF_MS`, `200`), `backoff_max_ms`
  (`SPEAK_RETRY_BACKOFF_MAX_MS`, `5000`), `multiplier`
  (`SPEAK_RETRY_MULTIPLIER`, `2.0`), `jitter` (`SPEAK_RETRY_JITTER`, `true`),
  and `retry_on` (`SPEAK_RETRY_ON`, default `connect + timeout + 5xx + 429`).
  This section is the TOML projection of the `RetryPolicy` domain value object.
- `[general]` — `quiet`, `json`, `color`, `temp_dir`, `log`, `config_path`.

### Universal env-overridability (no magic numbers)

Every tunable in the catalog above — timeouts, pool sizes, chunk sizes, buffer
frames, silence thresholds, sample rates, ffmpeg knobs, and the entire `[retry]`
policy — is env-overridable via a `SPEAK_*` variable with a code default, under
the same `flag > env > toml > default` precedence (FR-18). There are no
hardcoded magic numbers for tunables anywhere in the implementation; the Validate
phase asserts this (grep/review) and `config show` lists every value with its
origin, so any effective value is traceable to its source.

### Consequences

- Good: one rule, total coverage, and a `config show` that explains origin;
  `config init` emits a commented file; the `ConfigProvider` port keeps the
  rest of the system unaware of TOML/env mechanics.
- Bad: a large catalog to keep in sync across the schema, the template, and
  `config show`; the cross-consistency validator and CUE schema must mirror it.
  The mirror lives in `docs/arch/schemas/config.cue` (`#Config` and the
  `#Server`/`#Tts`/`#Asr`/`#AudioOutput`/`#AudioInput`/`#Ffmpeg`/`#Realtime`/
  `#Daemon`/`#Http`/`#Retry`/`#General` section types, plus the `#GenParams`
  value object for `[tts.gen]` and the `#RetryPolicy`/`#RetryOn` value objects
  for `[retry]`); changes to this catalog must update that file in the same
  change.
