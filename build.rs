//! Build script.
//!
//! On macOS, embed `Info.plist` into the binary's `__TEXT,__info_plist` section
//! so the executable declares `NSAudioCaptureUsageDescription` /
//! `NSMicrophoneUsageDescription` and can be a TCC subject — required for the
//! native Core Audio system-output tap (`--source output`, ADR-0015) to receive
//! audio rather than silently muted (zeroed) buffers.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        println!("cargo:rerun-if-changed=Info.plist");
        println!(
            "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{manifest}/Info.plist"
        );
    }
}
