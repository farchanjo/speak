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

  Scenario: Forward through the daemon with one-shot fallback
    Given the daemon is running on the Unix socket
    When I run "speak say warm"
    Then the request is forwarded over the socket to the warm pooled client
    And when no daemon is running the command falls back to a one-shot client
