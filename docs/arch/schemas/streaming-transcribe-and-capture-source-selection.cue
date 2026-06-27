// DDD role: ValueObject

package schemas

// #CaptureDirection selects which side of the device the capture reads from:
// an audio input (mic / line-in) or the host's output (system / sound-card
// playback, captured via the native tap). ADR-0015.
// DDD role: ValueObject
#CaptureDirection: "input" | "output"

// #CaptureSource is the Strategy selector for a live/record capture (FR-3..FR-6,
// ADR-0015). `device` absent = the system default for the direction (default
// input, or the system output mix); `channel` absent = downmix all channels
// (ADR-0013). It carries no framework type.
// DDD role: ValueObject
#CaptureSource: {
	direction: #CaptureDirection | *"input"
	// AudioDeviceID of the device to capture; absent = default for `direction`.
	device?: uint32
	// 0-based capture channel within the source; absent = downmix all.
	channel?: uint16
}

// #StreamingTranscribe models a `transcribe --stream` invocation (FR-1, FR-2,
// FR-7, ADR-0014): a live capture source, the silence gate, the chunk length,
// and the optional source-language hint. The transcript is streamed from the
// realtime SSE endpoint with translate=false (transcript frames only).
// DDD role: ValueObject
#StreamingTranscribe: {
	source:    #CaptureSource
	chunkSecs: number & >0 | *5.0
	vad:       bool | *true
	// Linear RMS floor below which a chunk is treated as silence.
	silenceFloor: number & >=0 | *0.0
	language?:    string & !=""
}
