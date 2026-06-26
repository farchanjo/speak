// DDD role: InfrastructureLayer
//
// Config catalog + remaining domain value objects.
// This file mirrors the ADR-0006 configuration catalog (the cross-consistency
// validator and CUE schema must mirror it) and adds the domain value objects
// (#GenParams, #PcmBuffer) and the adapters/sse wire DTO (#RealtimeFrame) that
// the domain schema file does not model. The `[retry]` (#Retry) and `[http]`
// (#Http) sections mirror the resilience + chat-MT catalog of ADR-0006; #Retry
// is the TOML projection of the `#RetryPolicy` domain value object.
//
// The config section types carry the `InfrastructureLayer` DDD role: they mirror
// the flat ADR-0006 TOML catalog, so the validator's domain calisthenics
// (wrap-primitives / small-entities) are intentionally relaxed here rather than
// inflating a configuration DTO into a deep domain model.

package schemas

// #GenParams is the value object of server generation-tuning knobs (FR-4 /
// ADR-0004 / tasks T013). Every field is optional (default unset). The only
// accepted CANONICAL key for step count is `num_step` (CLI alias `steps`);
// `num_steps` is NOT a valid key.
// DDD role: ValueObject
#GenParams: {
	num_step?:              int & >0 // alias: steps
	guidance_scale?:        number
	t_shift?:               number
	layer_penalty_factor?:  number
	position_temperature?:  number
	class_temperature?:     number
	denoise?:               number
	preprocess_prompt?:     bool
	postprocess_output?:    bool
	audio_chunk_duration?:  number & >0
	audio_chunk_threshold?: number
}

// #PcmSampleFormat enumerates the raw interleaved PCM sample formats the libav
// resampler targets (48 kHz stereo f32 for playback, 16 kHz mono s16 for ASR).
// Distinct from #SampleFormat, which names encoded container/response formats.
// DDD role: ValueObject
#PcmSampleFormat: "u8" | "s16" | "s32" | "f32"

// #PcmBuffer is decoded, interleaved PCM with its format descriptor (the unit
// passed between the libav decoder/resampler and the CoreAudio sink).
// DDD role: ValueObject
#PcmBuffer: {
	sampleRate:   int & >0
	channels:     int & >0
	sampleFormat: #PcmSampleFormat
	samples:      int & >=0 // total interleaved sample count
}

// #RealtimeFrameType is the SSE frame discriminator (ADR-0004).
// DDD role: ValueObject (adapter-level)
#RealtimeFrameType: "transcript" | "translation" | "audio" | "done" | "error"

// #RealtimeFrame is the adapters/sse wire DTO decoded from the realtime SSE
// stream `POST /v1/realtime/translate` (ADR-0004). It is an adapter-level
// DTO, NOT a domain type; it is modelled here only for schema completeness.
#RealtimeFrame: {
	type:       #RealtimeFrameType
	text?:      string
	audio_b64?: string
	format?:    #SampleFormat
	seq?:       int & >=0
}

// ---------------------------------------------------------------------------
// Configuration catalog (ADR-0006). Precedence: flag > env > toml > default.
// ---------------------------------------------------------------------------

// #Config is the typed configuration aggregate assembled by the config Builder.
// DDD role: InfrastructureLayer
#Config: {
	server:   #Server
	tts:      #Tts
	asr:      #Asr
	audio:    #Audio
	ffmpeg:   #Ffmpeg
	realtime: #Realtime
	daemon:   #Daemon
	http:     #Http
	retry:    #Retry
	general:  #General
}

// [server] — HTTP connection + warm pool.
#Server: {
	host:                    string & !="" | *"http://solaris:8800" // SPEAK_HOST
	api_key?:                string                                 // SPEAK_API_KEY
	timeout_secs?:           int & >0
	connect_timeout_secs?:   int & >0
	pool_max_idle_per_host?: int & >=0
	pool_idle_timeout_secs?: int & >=0
	tcp_keepalive_secs?:     int & >=0
	http2?:                  bool
	user_agent?:             string
}

// [tts] + [tts.gen].
#Tts: {
	language:  #Language | *"pt-BR"
	voice:     string & !="" | *"alloy"
	format:    #SampleFormat | *"mp3"
	model:     string & !="" | *"tts-1"
	speed?:    #Speed
	instruct?: string
	native:    bool | *false
	gen:       #GenParams
}

// [asr].
#Asr: {
	model:    string & !="" | *"whisper-1"
	language: #Language | *"auto"
	format:   "json" | "text" | "srt" | "vtt" | "verbose_json" | *"text"
}

// [audio.output] + [audio.input].
#Audio: {
	output: #AudioOutput
	input:  #AudioInput
}

// [audio.output].
// `rate` is the playback DEVICE's nominal hardware sample rate requested from
// CoreAudio (the output node rate); `sample_rate` is the PCM sample rate the
// libav resampler targets before feeding the mixer. They differ when the
// device runs at a rate other than the decode target.
//
// `device` is either a single device NAME or a LIST of device names (the
// default fan-out set, FR-11 / ADR-0007). Modeling it as `string | [...string]`
// lets the TOML express multi-output, removing the prior asymmetry where only
// the CLI could; the repeatable `--output-device` flag overrides this default
// per invocation under the usual flag > env > toml precedence.
#AudioOutput: {
	device?:        string | [...string]
	volume?:        number & >=0 & <=1 // drives mainMixerNode.outputVolume
	rate?:          int & >0           // device nominal hardware rate
	sample_rate?:   int & >0           // resample target fed to the mixer
	channels?:      int & >0
	buffer_frames?: int & >0
	play:           bool | *true
}

// [audio.input]. `device` is a device NAME (resolved to an AudioDeviceID),
// matching [audio.output].device — never a numeric index.
#AudioInput: {
	device?:              string
	sample_rate:          int & >0 | *16000
	channels:             int & >0 | *1
	chunk_secs:           number & >0 | *5
	silence_threshold_db: number | *-40
	vad?:                 bool
}

// [ffmpeg].
#Ffmpeg: {
	threads?:          int & >=0
	resampler?:        string
	resample_quality?: string
	dither?:           bool
	sample_fmt?:       string
	log_level?:        string
	extra_filters?:    string
}

// [realtime]. `translate` (SPEAK_RT_TRANSLATE) toggles translate-vs-passthrough
// mode; `speak` toggles whether the result is spoken back. They are distinct
// keys, not aliases.
#Realtime: {
	from?:      #Language
	to?:        #Language
	speak:      bool | *true
	chunk_secs: number & >0 | *5
	translate:  bool | *true // SPEAK_RT_TRANSLATE
}

// [daemon]. `autostart=true` lets a one-shot CLI invocation auto-launch the
// daemon binary on first use (vs silently running one-shot when false).
#Daemon: {
	socket:        string & !="" | *"~/.speak/speak.sock"
	idle_timeout?: int & >=0
	autostart:     bool | *false
}

// [http] — non-OpenAI chat-MT endpoint and the save directory for `-o`/saved
// output. `translate_url` (with `translate_model`) enables arbitrary `--to`
// targets in the realtime pipeline (FR-8); without it the client degrades to
// the source transcript. All three are env-overridable (SPEAK_TRANSLATE_URL /
// SPEAK_TRANSLATE_MODEL / SPEAK_SAVE_DIR).
#Http: {
	translate_url?:   string & !="" // SPEAK_TRANSLATE_URL
	translate_model?: string & !="" // SPEAK_TRANSLATE_MODEL
	save_dir?:        string & !="" // SPEAK_SAVE_DIR
}

// [retry] — configurable exponential-backoff + jitter resilience policy
// (FR-17 / ADR-0004 / ADR-0006). Every network call is wrapped by it. This is
// the TOML projection of the #RetryPolicy domain value object; every field is
// env-overridable so there are no hardcoded magic numbers (FR-18).
#Retry: {
	max_retries:        int & >=0 | *3   // SPEAK_RETRY_MAX
	backoff_initial_ms: int & >0 | *200  // SPEAK_RETRY_BACKOFF_MS
	backoff_max_ms:     int & >0 | *5000 // SPEAK_RETRY_BACKOFF_MAX_MS
	multiplier:         number & >0 | *2.0
	jitter:             bool | *true // SPEAK_RETRY_JITTER
	jitter_seed?:       int & >=0    // SPEAK_RETRY_JITTER_SEED; fixes the RNG for reproducible jitter
	// SPEAK_RETRY_ON; default retries connect + timeout + 5xx + 429.
	retry_on: *["connect", "timeout", "5xx", "429"] | [...#RetryOn]
}

// [general] + top-level extras. (`translate_url`/`translate_model`/`save_dir`
// moved to [http]; retry/backoff superseded by the [retry] policy section.)
#General: {
	quiet:        bool | *false
	json:         bool | *false
	color?:       bool
	temp_dir?:    string
	log?:         string
	config_path?: string
}
