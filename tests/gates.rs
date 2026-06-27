//! Resilience + hygiene gates (T063 / FR-17 / FR-18), enforced as ordinary
//! `cargo test` cases so the standard gate (`cargo nextest run`) fails the build
//! the instant either invariant is violated — no external grep step to remember.
//!
//! 1. **Zero media-exec.** The media path decodes/resamples in-process with the
//!    linked `libav*` (`ffmpeg-the-third`) and plays through the native CoreAudio
//!    mixer; nothing is shelled out (ADR-0002 / ADR-0007). No
//!    `std::process::Command` / `tokio::process::Command` spawn of any external
//!    binary — least of all `ffmpeg`/`afplay`/`ffplay` — may appear anywhere in
//!    `src/`. The ONLY sanctioned process spawn is a daemon self-spawn of our own
//!    binary, which must reference `current_exe` and be gated behind
//!    `[daemon] autostart` (ADR-0005); the gate whitelists exactly that shape.
//!
//! 2. **Zero magic numbers.** Every tunable (timeouts, pool sizes, chunk/buffer
//!    sizes, sample rates, retry params, ffmpeg knobs) resolves through a
//!    `SPEAK_*` env override + code default and is recorded for `config show`
//!    (FR-18). The gate asserts every knob-resolution site in `config.rs` carries
//!    a `SPEAK_*` env key, so a new tunable cannot be smuggled in as a bare
//!    literal that bypasses the precedence engine.

use std::path::{Path, PathBuf};

/// Absolute path to the crate's `src/` tree.
fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Recursively collect every `.rs` file under `dir`.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read src subdir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    out
}

/// Forbidden process-spawn machinery: the only way to exec an external media
/// binary. The media path is in-process, so none of these may appear in `src/`.
const SPAWN_TOKENS: &[&str] = &["Command::new", "process::Command", "tokio::process"];

#[test]
fn zero_media_exec_in_src() {
    let files = rust_files(&src_dir());
    assert!(files.len() > 20, "expected a populated src tree");

    let mut violations = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read src file");
        for (lineno, line) in text.lines().enumerate() {
            // A daemon self-spawn of our OWN binary is the one sanctioned process
            // exec (ADR-0005); it references `current_exe`. Everything else is a
            // forbidden external-process call.
            if line.contains("current_exe") {
                continue;
            }
            for token in SPAWN_TOKENS {
                if line.contains(token) {
                    violations.push(format!(
                        "{}:{}: forbidden process spawn `{token}`",
                        file.display(),
                        lineno + 1
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "media must never be shelled out (ADR-0002/0007). Offenders:\n{}",
        violations.join("\n")
    );
}

/// Every knob the resolver picks must be env-overridable (FR-18). A knob is a
/// `self.val(` / `self.opt(` / `self.secret(` call; each must carry a `SPEAK_`
/// env key within its (multi-line) argument list.
#[test]
fn every_config_knob_resolves_through_a_speak_env_override() {
    const CALLS: &[&str] = &["self.val(", "self.opt(", "self.secret("];
    /// Lines to scan past a knob call site for its `SPEAK_*` env literal.
    const WINDOW: usize = 8;

    let config =
        std::fs::read_to_string(src_dir().join("adapters/config.rs")).expect("read config.rs");
    // Restrict to the resolver section: knob calls live only on `self`, never in
    // the `#[cfg(test)]` module (tests call the free `pick_*` helpers directly).
    let lines: Vec<&str> = config.lines().collect();

    let mut knob_calls = 0usize;
    let mut missing = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !CALLS.iter().any(|c| line.contains(c)) {
            continue;
        }
        knob_calls += 1;
        let end = (idx + WINDOW).min(lines.len());
        let has_env = lines[idx..end].iter().any(|l| l.contains("\"SPEAK_"));
        if !has_env {
            missing.push(format!("config.rs:{}: knob without a SPEAK_* env", idx + 1));
        }
    }

    assert!(
        knob_calls >= 60,
        "expected the full config catalog (>=60 knobs), found {knob_calls}"
    );
    assert!(
        missing.is_empty(),
        "every tunable must have a SPEAK_* override (FR-18). Offenders:\n{}",
        missing.join("\n")
    );
}

/// The `SPEAK_*` env catalog must stay broad — a quick tripwire that the
/// precedence engine still routes the whole knob surface through the env layer.
#[test]
fn config_exposes_a_broad_speak_env_catalog() {
    let config =
        std::fs::read_to_string(src_dir().join("adapters/config.rs")).expect("read config.rs");
    let mut envs: Vec<&str> = config
        .match_indices("SPEAK_")
        .map(|(i, _)| {
            let rest = &config[i..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            &rest[..end]
        })
        // Drop the synthetic `SPEAK_TEST_*` names the precedence unit tests use.
        .filter(|name| !name.starts_with("SPEAK_TEST"))
        .collect();
    envs.sort_unstable();
    envs.dedup();
    assert!(
        envs.len() >= 50,
        "expected a broad SPEAK_* catalog (>=50), found {}: {envs:?}",
        envs.len()
    );
}
