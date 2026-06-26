Feature: speak CLI speech client for the solaris server
  As a CLI user of the speak binary
  I want text-to-speech, transcription, translation, a realtime pipeline,
  recording, device discovery, and layered configuration
  So that I can drive the OpenAI-compatible speech server from one command

  Background:
    Given a reachable OpenAI-compatible speech server at "$SPEAK_HOST"

  Scenario: Synthesize and play pt-BR speech by default
    When I run "speak say olá"
    Then the server synthesizes pt-BR audio
    And the audio plays through the native CoreAudio mixer
    And the exit code is 0

  Scenario: Apply a voice design from canonical tags
    When I run "speak say hi --instruct Female, Young Adult, British Accent"
    Then the server applies the voice design
    And the audio plays with exit code 0

  Scenario: Reject a non-canonical voice-design tag
    When I run "speak say hi --instruct sounds friendly"
    Then the command fails before sending the request
    And stderr explains that only canonical tags are accepted

  Scenario: Clone a saved voice
    Given a saved voice named "narrator"
    When I run "speak say hello --voice narrator"
    Then the server clones the saved voice for synthesis
    And the audio plays with exit code 0

  Scenario: Transcribe an audio file to text
    Given an audio file "a.mp3"
    When I run "speak transcribe a.mp3 --format text"
    Then stdout is the transcript text
    And the exit code is 0

  Scenario: Translate foreign-language audio to English
    Given foreign-language audio "f.mp3"
    When I run "speak translate f.mp3"
    Then stdout is the English translation
    And the exit code is 0

  Scenario: Live translate the microphone until interrupted
    Given a microphone is available
    When I run "speak realtime --from en --to pt-BR --translate"
    Then each utterance is transcribed, translated to pt-BR, printed, and spoken
    And the loop continues until Ctrl-C

  Scenario: Re-voice the microphone without translating
    Given a microphone is available
    When I run "speak realtime --no-translate --voice narrator"
    Then each utterance is transcribed and re-voiced as the saved voice
    And the audio plays locally

  Scenario: Fan output out to two devices from a single decode
    Given two output devices "A" and "B"
    When I run "speak say test --output-device A --output-device B"
    Then the decoded audio plays simultaneously on both devices
    And the path stays in-process with no child process

  Scenario: Use the default host with no config and no flags
    Given no config file and no environment overrides
    When I run "speak say oi"
    Then the default host "http://solaris:8800" is used
    And the command succeeds

  Scenario: Report each config value and its origin
    Given a value set in the TOML and overridden by an environment variable
    When I run "speak config show"
    Then each effective value is printed with its origin flag, env, toml, or default

  Scenario: Retry a transient server error with exponential backoff
    Given the server returns a transient 5xx then succeeds
    When I run "speak say oi"
    Then the client retries with exponential backoff and jitter per the retry policy
    And the request ultimately succeeds with exit code 0

  Scenario: Do not retry a non-retryable client error
    Given the server returns a non-retryable 4xx
    When I run "speak say oi"
    Then the client does not retry
    And the command exits non-zero with the server error

  Scenario: Reconnect the realtime stream after an SSE drop
    Given a microphone is available
    And the realtime SSE stream drops mid-session
    When I run "speak realtime --translate"
    Then the client reconnects under the bounded retry policy
    And the live session continues until Ctrl-C

  Scenario: Override the retry policy from the environment
    Given "SPEAK_RETRY_MAX" is set to "5" in the environment
    When I run "speak config show"
    Then the retry "max_retries" value is "5" with origin "env"

  Scenario: Forward through the daemon with one-shot fallback
    Given the daemon is running on the Unix socket
    When I run "speak say warm"
    Then the request is forwarded over the socket to the warm pooled client
    And when no daemon is running the command falls back to a one-shot client

  Scenario: Save synthesized audio to a file without playing
    When I run "speak say report -o out.mp3 --no-play"
    Then the decoded audio is written to "out.mp3"
    And nothing plays through the speakers
    And the exit code is 0

  Scenario: Pass generation parameters through to the server
    When I run "speak say tuned --set num_step=24 --set guidance_scale=3"
    Then the gen-params are validated and forwarded to the server
    And the exit code is 0

  Scenario: Reject an unknown generation-parameter key
    When I run "speak say tuned --set num_steps=24"
    Then the command fails before sending the request
    And stderr explains that "num_steps" is not a valid gen-param key

  Scenario: Synthesize through the native tts endpoint
    When I run "speak say nativo --native"
    Then the request goes to the native "/tts" endpoint
    And the audio plays with exit code 0

  Scenario: Echo the microphone then re-voice it
    Given a microphone is available
    When I run "speak realtime --echo --instruct Female, Young Adult, British Accent"
    Then the raw captured audio is played back
    And then each utterance is re-voiced via TTS
    And the loop continues until Ctrl-C

  Scenario: Degrade realtime translation when no chat-MT endpoint is set
    Given a microphone is available
    And no "translate_url" is configured
    When I run "speak realtime --from en --to ja --translate"
    Then the client falls back to the source transcript
    And stderr notes that an arbitrary target needs "translate_url"

  Scenario: Record the microphone to a FLAC file
    Given a microphone is available
    When I run "speak record --output take.flac --format flac --duration 5"
    Then the captured audio is encoded in-process to "take.flac"
    And the path stays in-process with no child process
    And the exit code is 0

  Scenario: List input and output audio devices as JSON
    When I run "speak devices --json"
    Then stdout lists the input and output devices with their AudioDeviceIDs
    And the exit code is 0

  Scenario: Add, list, and remove a saved voice
    Given an audio sample "sample.wav"
    When I run "speak voices add narrator --audio sample.wav"
    Then the voice "narrator" is registered on the server
    And "speak voices list" includes "narrator"
    And "speak voices rm narrator" deletes it

  Scenario: Check server health
    When I run "speak health"
    Then the client queries "GET /health"
    And reports the server status with exit code 0

  Scenario: Report local acceleration and environment
    When I run "speak check"
    Then stdout reports OS, arch, CPU cores, libav hwdevices, and the hwaccel policy
    And the exit code is 0

  Scenario: Emit a shell completion script
    When I run "speak completions zsh"
    Then stdout is a valid zsh completion script
    And the exit code is 0

  Scenario: Write a fully commented config template
    Given no config file exists
    When I run "speak config init"
    Then a commented "~/.speak/config.toml" is written
    And the exit code is 0

  Scenario: Print the resolved config path
    When I run "speak config path"
    Then stdout is the resolved "~/.speak/config.toml" path
    And the exit code is 0

  Scenario: Stop and query the daemon
    Given the daemon is running on the Unix socket
    When I run "speak daemon status"
    Then stdout reports the daemon as running
    And "speak daemon stop" terminates it
