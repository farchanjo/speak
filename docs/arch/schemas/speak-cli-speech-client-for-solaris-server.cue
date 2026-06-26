// DDD role: ValueObject

package schemas

// #Language is a BCP-47-ish language hint (e.g. "pt-BR", "en", "auto").
// DDD role: ValueObject
#Language: string & !=""

// #SampleFormat enumerates the canonical response/container formats.
// DDD role: ValueObject
#SampleFormat: "mp3" | "opus" | "aac" | "flac" | "wav" | "pcm"

// #SpeechText is the non-empty input to synthesize.
// DDD role: ValueObject
#SpeechText: string & !=""

// #Speed is the playback/synthesis speed multiplier.
// DDD role: ValueObject
#Speed: number & >0

// #VoiceMode is the choice of how output sounds: design, clone, or (neither
// set) the server default/auto. At most one of `design`/`clone` is present.
// The referenced voice types live in voice.cue (same package).
// DDD role: ValueObject
#VoiceMode: {
	design?: #VoiceDesign
	clone?:  #VoiceClone
}

// #SpeechSpec is the aggregate describing one synthesis request
// (FR-1..FR-4 / tasks T014): input + voice mode + format + language + speed +
// gen-params, plus the `model`, `native` endpoint toggle, and optional
// `--duration`. It is value-equal (no identity). #GenParams lives in config.cue.
// DDD role: Aggregate
#SpeechSpec: {
	input:      #SpeechText
	language:   #Language
	format:     #SampleFormat
	speed:      #Speed | *1.0
	voice:      #VoiceMode
	model:      string & !="" | *"tts-1"
	native:     bool | *false
	duration?:  number & >0
	genParams?: #GenParams
}

// #RealtimeMode is the realtime pipeline strategy (FR-8 / ADR-0004).
// DDD role: ValueObject
#RealtimeMode: "translate" | "no-translate" | "echo"

// #ConfigOrigin labels where an effective config value came from (FR-13).
// DDD role: ValueObject
#ConfigOrigin: "flag" | "env" | "toml" | "default"

// #RetryOn classifies which network failures the retry policy retries
// (FR-17 / ADR-0004 / ADR-0006). The default retried set is the union of all
// four: connection failures, timeouts, server 5xx, and 429 throttling.
// DDD role: ValueObject
#RetryOn: "connect" | "timeout" | "5xx" | "429"

// #RetryPolicy is the domain value object describing the configurable
// exponential-backoff + jitter retry strategy (FR-17 / ADR-0004). It is pure
// data injected at the composition root and applied to every network call
// (synthesize, transcribe, translate, voice CRUD, daemon forward, SSE reconnect)
// via the `RetryPolicy` port (a GoF Strategy). It mirrors the `[retry]` config
// section (#Retry in config.cue) one-to-one. It is value-equal (no identity).
// DDD role: ValueObject
#RetryPolicy: {
	maxRetries:       int & >=0  // attempts beyond the first
	backoffInitialMs: int & >0   // first delay before retry
	backoffMaxMs:     int & >0    // delay ceiling after growth
	multiplier:       number & >0 // geometric growth factor per attempt
	jitter:           bool | *true
	retryOn: [...#RetryOn] & [_, ...] // non-empty classification set
}
