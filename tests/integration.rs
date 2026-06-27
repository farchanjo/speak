//! Live-server integration tests (feature-gated, off by default).
//!
//! Enable with `cargo test --features integration` when the OpenAI-compatible
//! speech server is reachable (default `http://solaris:8800`, overridable via
//! `SPEAK_HOST`). Each test first probes the server's TCP port; when it is not
//! reachable the test prints a skip note and returns OK rather than failing, so
//! the suite is safe to run anywhere.
#![cfg(feature = "integration")]

use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Output};
use std::time::Duration;

/// Default server, matching the `[server] host` code default.
const DEFAULT_HOST: &str = "http://solaris:8800";

/// Resolve the configured host (env override or default) to a `host:port`.
fn host_port() -> (String, String) {
    let host = std::env::var("SPEAK_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_owned());
    let stripped = host
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    let authority = stripped.split('/').next().unwrap_or(stripped);
    let with_port = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:8800")
    };
    (host, with_port)
}

/// True when the server TCP port accepts a connection within a short timeout.
fn server_reachable(authority: &str) -> bool {
    let Ok(mut addrs) = authority.to_socket_addrs() else {
        return false;
    };
    addrs.any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(800)).is_ok())
}

fn run(host: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_speak"))
        .args(args)
        .env("SPEAK_LOG", "off")
        .env("SPEAK_HOST", host)
        .output()
        .expect("spawn speak binary")
}

/// Run `body` only when the server is up; otherwise emit a skip note.
fn with_server(name: &str, body: impl FnOnce(&str)) {
    let (host, authority) = host_port();
    if server_reachable(&authority) {
        body(&host);
    } else {
        eprintln!("SKIP {name}: server {authority} unreachable (set SPEAK_HOST to run)");
    }
}

#[test]
fn health_reports_server_status() {
    with_server("health", |host| {
        // Plain run: the Presenter console adapter emits a key/value report
        // (health is routed through the Presenter port, not raw println!).
        let out = run(host, &["health"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("healthy"), "expected a health report: {text}");
        assert!(text.contains("models"), "expected a models row: {text}");

        // `--json` (FR-16): the same result as a single machine-readable object.
        let json = run(host, &["--json", "health"]);
        assert!(
            json.status.success(),
            "{}",
            String::from_utf8_lossy(&json.stderr)
        );
        let body = String::from_utf8_lossy(&json.stdout);
        let value: serde_json::Value = serde_json::from_str(body.trim())
            .unwrap_or_else(|e| panic!("invalid JSON {body}: {e}"));
        assert!(
            value["entries"]["healthy"].is_string(),
            "expected entries.healthy: {body}"
        );
    });
}

#[test]
fn voices_list_succeeds() {
    with_server("voices_list", |host| {
        let out = run(host, &["voices", "list"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
}

#[test]
fn say_writes_audio_file_without_playing() {
    with_server("say_to_file", |host| {
        let path = std::env::temp_dir().join("speak-it-say.mp3");
        let _ = std::fs::remove_file(&path);
        let out = run(
            host,
            &[
                "say",
                "hello from the integration suite",
                "-o",
                path.to_str().unwrap(),
                "--no-play",
            ],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let meta = std::fs::metadata(&path).expect("audio file written");
        assert!(meta.len() > 0, "audio file is empty");
    });
}

#[test]
fn say_then_transcribe_round_trips_text() {
    with_server("round_trip", |host| {
        let phrase = "integration round trip";
        let path = std::env::temp_dir().join("speak-it-roundtrip.wav");
        let _ = std::fs::remove_file(&path);
        let say = run(
            host,
            &[
                "say",
                phrase,
                "--format",
                "wav",
                "-o",
                path.to_str().unwrap(),
                "--no-play",
                "--lang",
                "en",
            ],
        );
        assert!(
            say.status.success(),
            "{}",
            String::from_utf8_lossy(&say.stderr)
        );
        let asr = run(
            host,
            &[
                "transcribe",
                path.to_str().unwrap(),
                "--format",
                "text",
                "--language",
                "en",
            ],
        );
        assert!(
            asr.status.success(),
            "{}",
            String::from_utf8_lossy(&asr.stderr)
        );
        let got = String::from_utf8_lossy(&asr.stdout).to_lowercase();
        // ASR is fuzzy; assert a salient content word survives the round trip.
        assert!(
            got.contains("round") || got.contains("trip"),
            "transcript was: {got}"
        );
    });
}

#[test]
fn voice_design_say_is_accepted() {
    with_server("voice_design", |host| {
        let path = std::env::temp_dir().join("speak-it-design.mp3");
        let _ = std::fs::remove_file(&path);
        let out = run(
            host,
            &[
                "say",
                "designed voice",
                "--instruct",
                "Female, Young Adult, British Accent",
                "-o",
                path.to_str().unwrap(),
                "--no-play",
            ],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
}
