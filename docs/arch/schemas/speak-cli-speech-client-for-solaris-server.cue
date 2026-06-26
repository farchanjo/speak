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

// #SpeechSpec is the value object describing one synthesis request (FR-1..FR-4);
// it is value-equal (no identity). Generation-parameter and server tuning knobs
// are catalogued in ADR-0006.
// DDD role: ValueObject
#SpeechSpec: {
	input:    #SpeechText
	language: #Language
	format:   #SampleFormat
	speed:    #Speed | *1.0
	voice:    #VoiceMode
}

// #RealtimeMode is the realtime pipeline strategy (FR-8 / ADR-0004).
// DDD role: ValueObject
#RealtimeMode: "translate" | "no-translate" | "echo"

// #ConfigOrigin labels where an effective config value came from (FR-13).
// DDD role: ValueObject
#ConfigOrigin: "flag" | "env" | "toml" | "default"
