// DDD role: ValueObject

package schemas

// #VoiceName is the identity of a saved, cloneable voice.
// DDD role: ValueObject
#VoiceName: string & !=""

// #RefText is optional reference text guiding a clone or design.
// DDD role: ValueObject
#RefText: string & !=""

// #VoiceDesignTag is one canonical voice-design tag; free text is rejected
// (FR-3 / ADR-0004). These are the only 23 accepted English tags.
// DDD role: ValueObject
#VoiceDesignTag: "male" | "female" | "child" | "teenager" | "young adult" |
	"middle-aged" | "elderly" | "very low pitch" | "low pitch" |
	"moderate pitch" | "high pitch" | "very high pitch" | "whisper" |
	"american accent" | "australian accent" | "british accent" |
	"canadian accent" | "chinese accent" | "indian accent" |
	"japanese accent" | "korean accent" | "portuguese accent" |
	"russian accent"

// #VoiceDesign is a non-empty list of canonical tags (the --instruct value).
// DDD role: ValueObject
#VoiceDesign: {
	tags: [...#VoiceDesignTag] & [_, ...]
}

// #StandardVoice is a built-in, server-provided standard voice referenced by a
// well-known name (e.g. the `[tts].voice` default `alloy`), NOT a saved clone.
// It disambiguates the `--voice` collision: `[tts].voice` defaults to a standard
// OpenAI-style voice name, while a `--voice <saved-name>` whose name matches a
// registered clone resolves to #VoiceClone. Resolution order (FR-2): if the name
// matches a saved voice it is a clone; otherwise it is passed through as a
// standard voice name for the server to resolve.
// DDD role: ValueObject
#StandardVoice: {
	name: #VoiceName
}

// #VoiceClone references a saved voice, optionally with reference text.
// DDD role: ValueObject
#VoiceClone: {
	name:     #VoiceName
	refText?: #RefText
}

// #Voice is a saved server-side cloneable voice; identity is its name.
// DDD role: Entity
#Voice: {
	id:       #VoiceName
	refText?: #RefText
}
