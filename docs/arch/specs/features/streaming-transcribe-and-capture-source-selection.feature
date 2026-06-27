Feature: Streaming transcribe and capture source selection
  As a CLI user of the speak binary
  I want a live streaming transcript and a selectable capture source
  So that I can transcribe my microphone or the host's own output, hands-free

  Background:
    Given a reachable OpenAI-compatible speech server at "$SPEAK_HOST"
    And the server exposes the realtime SSE endpoint

  Scenario: Stream a live transcript from the microphone
    When I run "speak transcribe --stream"
    Then the microphone is captured live in chunks
    And each server transcript frame is printed as a line
    And no audio is re-voiced or played
    And pressing Ctrl-C stops the loop with exit code 0

  Scenario: Streaming transcribe ignores re-voiced audio frames
    Given the server streams transcript and audio frames for translate=false
    When I run "speak transcribe --stream"
    Then only the transcript frames are surfaced
    And the audio frames are discarded without playback

  Scenario: Stream a transcript of the system output via the native tap
    Given the host is playing audio
    And the operating system supports the native Core Audio tap
    When I run "speak transcribe --stream --source output"
    Then the system output is captured directly with no hardware loopback
    And the transcript of the playing audio is printed incrementally

  Scenario: Output capture fails clearly when permission is denied
    Given macOS audio-capture permission is denied
    When I run "speak transcribe --stream --source output"
    Then the command fails before streaming
    And stderr explains the denial and names the BlackHole fallback

  Scenario: Capture a routed virtual-loopback device through the input source
    Given a BlackHole device is routed from the system output
    When I run "speak transcribe --stream --source input -d 42"
    Then the routed output is captured through the input source path
    And the transcript is printed incrementally

  Scenario: Record the system output to a file
    Given the host is playing audio
    When I run "speak record --source output -o sys.wav"
    Then a WAV file of the system output is written
    And the exit code is 0

  Scenario: Realtime translation of the system output
    Given the host is playing foreign-language audio
    When I run "speak realtime --translate --to en --source output"
    Then the system output is translated and re-voiced live

  Scenario: The capture source is reported with its config origin
    Given "[audio.capture].source" is set to "output" in config
    When I run "speak config show"
    Then the value "output" is reported with origin "toml"
