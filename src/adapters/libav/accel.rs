//! Local hardware-acceleration detection for libav (part of the `libav` driven
//! adapter — the only layer where `ffmpeg-the-third` appears, ADR-0001/ADR-0003).
//!
//! `speak` only handles **audio** (TTS/ASR). NVIDIA NVENC/NVDEC and other GPU
//! codecs accelerate *video*, not audio, and on this client the only GPU is
//! the server's (it runs TTS/ASR inference) — so there is no GPU audio-decode
//! path. The applicable *local* acceleration here is:
//!   * all CPU cores via libav frame threading, and
//!   * Apple AudioToolbox audio decoders (`*_at`) on macOS.
//!
//! `probe()` reports what is available; `resolve_decoder()` applies the policy
//! (`SPEAK_HWACCEL=auto|off|<decoder-name>`) per stream.

use std::ffi::CStr;

use ff::ffi;
use ffmpeg_the_third as ff;

/// Environment variable that overrides hardware-acceleration auto-detect.
pub const ENV_HWACCEL: &str = "SPEAK_HWACCEL";

/// AudioToolbox decoders worth probing for on macOS.
const AT_DECODERS: &[&str] = &[
    "aac_at",
    "ac3_at",
    "eac3_at",
    "alac_at",
    "mp1_at",
    "mp2_at",
    "mp3_at",
    "amrnb_at",
    "gsm_ms_at",
    "ilbc_at",
    "pcm_mulaw_at",
    "pcm_alaw_at",
];

/// Resolved acceleration policy.
#[derive(Debug, Clone)]
pub enum Policy {
    /// Auto-detect and use the best available local decoder.
    Auto,
    /// Disable hardware decoders; use the default software decoder.
    Off,
    /// Force a specific libav decoder by name.
    Named(String),
}

/// Read the policy from `SPEAK_HWACCEL` (default: auto).
#[must_use]
pub fn policy() -> Policy {
    match std::env::var(ENV_HWACCEL).ok().as_deref().map(str::trim) {
        None | Some("") | Some("auto") => Policy::Auto,
        Some("off" | "none" | "false") => Policy::Off,
        Some(name) => Policy::Named(name.to_owned()),
    }
}

/// Pick a libav decoder name for `default_name` (e.g. `mp3`) under the policy,
/// or `None` to use the default software decoder.
#[must_use]
pub fn resolve_decoder(default_name: &str) -> Option<String> {
    match policy() {
        Policy::Off => None,
        Policy::Named(name) => Some(name),
        Policy::Auto => {
            let candidate = format!("{default_name}_at");
            if cfg!(target_os = "macos") && ff::codec::decoder::find_by_name(&candidate).is_some() {
                Some(candidate)
            } else {
                None
            }
        }
    }
}

/// A snapshot of the host and its available local acceleration.
#[derive(Debug)]
pub struct Report {
    /// Operating system (`std::env::consts::OS`).
    pub os: String,
    /// CPU architecture.
    pub arch: String,
    /// Logical CPU cores available for threading.
    pub cpu_cores: usize,
    /// `libavcodec` version string.
    pub libavcodec: String,
    /// libav hardware device types compiled in (mostly video).
    pub hwdevice_types: Vec<String>,
    /// AudioToolbox audio decoders actually present.
    pub audiotoolbox_decoders: Vec<String>,
    /// Effective policy string.
    pub policy: String,
}

/// Probe the host for OS info and available local acceleration.
#[must_use]
pub fn probe() -> Report {
    let _ = ff::init();
    Report {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        cpu_cores: std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        libavcodec: version_string(unsafe { ffi::avcodec_version() }),
        hwdevice_types: hwdevice_types(),
        audiotoolbox_decoders: available_at_decoders(),
        policy: format!("{:?}", policy()),
    }
}

fn version_string(packed: u32) -> String {
    format!(
        "{}.{}.{}",
        packed >> 16,
        (packed >> 8) & 0xff,
        packed & 0xff
    )
}

fn hwdevice_types() -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: iterating libav's static hwdevice type table; each name pointer is
    // a static C string owned by libav.
    unsafe {
        let mut kind = ffi::av_hwdevice_iterate_types(ffi::AVHWDeviceType::NONE);
        while kind != ffi::AVHWDeviceType::NONE {
            let name = ffi::av_hwdevice_get_type_name(kind);
            if !name.is_null()
                && let Ok(s) = CStr::from_ptr(name).to_str()
            {
                out.push(s.to_owned());
            }
            kind = ffi::av_hwdevice_iterate_types(kind);
        }
    }
    out
}

fn available_at_decoders() -> Vec<String> {
    AT_DECODERS
        .iter()
        .filter(|name| ff::codec::decoder::find_by_name(name).is_some())
        .map(|name| (*name).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testenv::ENV_LOCK;

    fn with_hwaccel<T>(value: Option<&str>, body: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(ENV_HWACCEL).ok();
        match value {
            // TODO: Audit that the environment access only happens in single-threaded code.
            Some(v) => unsafe { std::env::set_var(ENV_HWACCEL, v) },
            // TODO: Audit that the environment access only happens in single-threaded code.
            None => unsafe { std::env::remove_var(ENV_HWACCEL) },
        }
        let out = body();
        match prev {
            // TODO: Audit that the environment access only happens in single-threaded code.
            Some(v) => unsafe { std::env::set_var(ENV_HWACCEL, v) },
            // TODO: Audit that the environment access only happens in single-threaded code.
            None => unsafe { std::env::remove_var(ENV_HWACCEL) },
        }
        out
    }

    #[test]
    fn policy_defaults_to_auto() {
        with_hwaccel(None, || assert!(matches!(policy(), Policy::Auto)));
        with_hwaccel(Some(""), || assert!(matches!(policy(), Policy::Auto)));
        with_hwaccel(Some("auto"), || assert!(matches!(policy(), Policy::Auto)));
    }

    #[test]
    fn policy_off_aliases() {
        for v in ["off", "none", "false"] {
            with_hwaccel(Some(v), || assert!(matches!(policy(), Policy::Off)));
        }
    }

    #[test]
    fn policy_named_decoder() {
        with_hwaccel(Some("mp3_at"), || match policy() {
            Policy::Named(name) => assert_eq!(name, "mp3_at"),
            other => panic!("expected Named, got {other:?}"),
        });
    }

    #[test]
    fn resolve_decoder_off_yields_software_default() {
        with_hwaccel(Some("off"), || assert_eq!(resolve_decoder("mp3"), None));
    }

    #[test]
    fn resolve_decoder_named_forces_choice() {
        with_hwaccel(Some("custom_dec"), || {
            assert_eq!(resolve_decoder("mp3"), Some("custom_dec".to_owned()));
        });
    }

    #[test]
    fn version_string_unpacks_semver() {
        // libav packs version as (major << 16) | (minor << 8) | micro.
        assert_eq!(version_string((62 << 16) | (3 << 8) | 100), "62.3.100");
    }
}
