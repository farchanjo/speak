//! CLI surface tests: drive the real compiled `speak` binary and assert on its
//! exit code, stdout, and stderr. These never touch the network — they exercise
//! argument parsing, value-enum validation, completions, the design catalog,
//! and config path/origin reporting only.
//!
//! Cargo exports the built binary path as `CARGO_BIN_EXE_speak`, so no extra
//! crate (assert_cmd) is required.

use std::process::{Command, Output};

/// Run the `speak` binary with `args` and a hermetic env: every test gets a
/// throwaway `SPEAK_CONFIG`/`SPEAK_HOME`/`SPEAK_LOG=off` so it never reads the
/// developer's real `~/.speak` config or writes log files.
/// `SPEAK_*` knobs that could leak from the developer's shell and skew origin
/// assertions; cleared on every invocation for a hermetic baseline.
const LEAKY_VARS: &[&str] = &[
    "SPEAK_HOST",
    "SPEAK_API_KEY",
    "SPEAK_LANG",
    "SPEAK_VOICE",
    "SPEAK_FORMAT",
    "SPEAK_RETRY_MAX",
];

fn base_command(args: &[&str]) -> Command {
    let tmp = std::env::temp_dir().join(format!("speak-cli-test-{}", std::process::id()));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_speak"));
    cmd.args(args)
        .env("SPEAK_LOG", "off")
        .env("SPEAK_HOME", &tmp)
        .env("SPEAK_CONFIG", tmp.join("config.toml"));
    for v in LEAKY_VARS {
        cmd.env_remove(v);
    }
    cmd
}

fn run(args: &[&str]) -> Output {
    base_command(args).output().expect("spawn speak binary")
}

fn run_with(env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = base_command(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn speak binary")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn version_flag_prints_name_and_exits_zero() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("speak"), "{}", stdout(&out));
}

#[test]
fn help_flag_lists_subcommands() {
    let out = run(&["--help"]);
    assert!(out.status.success());
    let text = stdout(&out);
    for sub in [
        "say",
        "transcribe",
        "translate",
        "realtime",
        "voices",
        "daemon",
    ] {
        assert!(text.contains(sub), "help missing `{sub}`: {text}");
    }
}

#[test]
fn no_args_shows_help_and_fails() {
    // `arg_required_else_help` => running bare exits non-zero with usage.
    let out = run(&[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("Usage") || stdout(&out).contains("Usage"));
}

#[test]
fn completions_zsh_emits_a_script() {
    let out = run(&["completions", "zsh"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("#compdef speak") || text.contains("_speak"),
        "{text}"
    );
}

#[test]
fn completions_bash_emits_a_script() {
    let out = run(&["completions", "bash"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("speak"));
}

#[test]
fn invalid_audio_format_is_rejected_before_network() {
    // ValueEnum rejection: clap fails the parse (exit 2) without a request.
    let out = run(&["say", "hi", "--format", "wavx"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("invalid value"), "{}", stderr(&out));
}

#[test]
fn invalid_text_format_is_rejected() {
    let out = run(&["transcribe", "a.mp3", "--format", "bogus"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("invalid value"));
}

#[test]
fn record_rejects_unknown_format_and_requires_output() {
    // The record container ValueEnum rejects unknown formats (exit 2)...
    let bad = run(&[
        "record",
        "-o",
        "x.wav",
        "--duration",
        "1",
        "--format",
        "ogg",
    ]);
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("invalid value"), "{}", stderr(&bad));
    // ...and --output / --duration are required, so a bare `record` fails parse.
    let missing = run(&["record"]);
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("required") || stderr(&missing).contains("Usage"),
        "{}",
        stderr(&missing)
    );
}

#[test]
fn list_designs_prints_canonical_tags_offline() {
    // `say --list-designs` short-circuits before any transport/network work.
    let out = run(&["say", "--list-designs"]);
    assert!(out.status.success());
    let text = stdout(&out).to_lowercase();
    assert!(text.contains("british accent"));
    assert!(text.contains("whisper"));
}

#[test]
fn config_path_reports_the_resolved_override() {
    let out = run_with(
        &[("SPEAK_CONFIG", "/tmp/explicit-speak.toml")],
        &["config", "path"],
    );
    assert!(out.status.success());
    assert!(stdout(&out).contains("/tmp/explicit-speak.toml"));
}

#[test]
fn config_show_reports_value_with_env_origin() {
    // FR: a pure-env knob (no matching flag) surfaces with origin `env`.
    // `SPEAK_RETRY_MAX=7` is the canonical example from the spec.
    let out = run_with(&[("SPEAK_RETRY_MAX", "7")], &["config", "show"]);
    assert!(out.status.success());
    let text = stdout(&out);
    let line = text
        .lines()
        .find(|l| l.contains("retry.max_retries"))
        .unwrap_or("");
    assert!(line.contains('7'), "{line}");
    // The Presenter `config show` table carries the origin in its own column.
    assert!(line.contains("env"), "{line}");
}

#[test]
fn config_show_reports_flag_origin_for_global_flag_env() {
    // `--host` carries `env = SPEAK_HOST`, so clap fills the flag: origin `flag`.
    let out = run_with(
        &[("SPEAK_HOST", "http://probe-host:9100")],
        &["config", "show"],
    );
    assert!(out.status.success());
    let text = stdout(&out);
    let line = text
        .lines()
        .find(|l| l.contains("server.host"))
        .unwrap_or("");
    assert!(line.contains("http://probe-host:9100"), "{line}");
    assert!(line.contains("flag"), "{line}");
}

#[test]
fn config_show_reports_default_origin_without_overrides() {
    let out = run(&["config", "show"]);
    assert!(out.status.success());
    let text = stdout(&out);
    let host_line = text
        .lines()
        .find(|l| l.contains("server.host"))
        .unwrap_or("");
    assert!(host_line.contains("http://solaris:8800"), "{host_line}");
    assert!(host_line.contains("default"), "{host_line}");
}

#[test]
fn check_reports_host_and_acceleration_offline() {
    let out = run(&["check"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("os / arch"));
    assert!(text.contains("hwaccel policy"));
}

#[test]
fn unknown_subcommand_is_rejected() {
    let out = run(&["frobnicate"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("unrecognized") || stderr(&out).contains("unexpected"));
}
