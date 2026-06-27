---
status: accepted
date: 2026-06-26
deciders: [farchanjo]
consulted: []
informed: []
---

# Persistent daemon over a Unix domain socket

## Context and Problem Statement

Each one-shot `speak` invocation otherwise pays the cost of building an HTTP
client and establishing a TLS/TCP connection to the server before the first
byte of inference. For interactive and realtime use this connection-setup
latency is repeated on every command. We want a way to keep one warm,
pooled client alive across invocations so subsequent commands ride an already
established keep-alive socket.

## Decision Drivers

- Amortize client/connection setup across many short-lived CLI invocations.
- Keep the common case (no daemon running) working with zero configuration.
- Stream realtime SSE frames through to the foreground CLI without buffering.
- Local-only IPC; no network listener, no extra auth surface.

## Considered Options

- Option A — A `speak daemon` process holding one pooled async-openai/reqwest
  client, listening on a Unix domain socket at `~/.speak/speak.sock`; CLI
  commands forward requests to it with length-prefixed framing and stream SSE
  frames back, with automatic one-shot fallback when no daemon is present.
- Option B — A TCP localhost listener with a small auth token.
- Option C — No daemon; rebuild the client every invocation.

## Decision Outcome

Chosen option: "Option A".

- `speak daemon` runs the listener; `--foreground` runs it attached, `stop`
  and `status` control a running instance. The socket path defaults to
  `~/.speak/speak.sock` (config `[daemon].socket`), with `idle_timeout` and an
  `autostart` toggle.
- `[daemon].autostart` semantics: when `true`, a one-shot CLI invocation that
  finds no daemon on the socket spawns the daemon binary itself (detached,
  `speak daemon`), waits briefly for the socket to appear, and then forwards
  the request to it — so the first command pays the launch cost and every later
  command rides the warm client. When `false` (the default), the same
  invocation skips launching and simply runs the one-shot in-process client.
  `autostart` only affects whether the daemon is auto-launched; it never
  changes the request result, and `idle_timeout` later reaps an auto-started
  daemon after inactivity.
- Normal CLI commands attempt to connect to the socket and forward the
  use-case request using length-prefixed framing; realtime SSE frames are
  forwarded frame-by-frame so the foreground sees live output. If the socket is
  absent or stale, the command transparently falls back to a one-shot in-process
  client — identical behavior, slightly higher first-byte latency.
- The daemon is a driving adapter in the hexagonal model: it deserializes a
  framed request, invokes exactly the same application use case the CLI would,
  and serializes the result. No business logic lives in the socket layer.

### Consequences

- Good: warm connections across invocations; realtime streams pass through
  transparently; the no-daemon path needs no setup.
- Good: a Unix socket confines IPC to the local user with filesystem
  permissions; no TCP port or token to manage.
- Bad: a second process to supervise; framing and SSE pass-through add protocol
  surface that must be versioned alongside the use-case contract; macOS/Linux
  only (no Windows named-pipe path today).

### Realized protocol (T053)

The framed contract was implemented as a **port-level** operation protocol rather
than a raw HTTP proxy, which makes the "same use case as the CLI" guarantee
structural: the daemon holds one warm `SpeakFacade` (the retry-wrapped `openai`
adapter + a `HeadlessAudio` role + `libav`) and `dispatch` calls the very same
Facade methods (`say` with `play = false`, `transcribe`, `translate`, the voice
repository, `health`). Each message is two length-prefixed frames — a JSON
`Request`/`Reply` header plus a binary audio payload — and the `SpeechSpec`
crosses the boundary as a `SpeechSpecDto` so the domain stays serde-free
(ADR-0003). The CLI side is the symmetric `DaemonSpeechAdapter`, selected by the
composition root's `SpeechRole` Strategy (forward when a daemon is live, else
in-process), so the use cases never know which they hold.

Because the daemon is headless, **local audio is never forwarded**: `record` and
`realtime` capture and `say` playback always run in the foreground CLI. Realtime
therefore runs its loop in-process and forwards only its per-chunk network speech
ports to the warm daemon — superseding the original frame-by-frame SSE
pass-through (the realtime SSE stream itself, once the T036 eventsource adapter
lands, is consumed in the foreground and reconnects under the shared retry policy,
T046). Transient socket failures on the forward path retry under the same
`RetryPolicy` as the network ports.
