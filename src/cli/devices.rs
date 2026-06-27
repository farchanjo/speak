//! `devices` handler (T056): enumerate input/output audio devices (FR-10).
//!
//! A thin CLI adapter that reads the `coreaudio` device-enumeration adapter
//! directly (no dedicated use case, per ADR-0003) and prints each device's
//! `AudioDeviceID`, name, channels, rate, and UID — or a JSON array with `--json`.

use anyhow::Result;

use speak::adapters::coreaudio;
use speak::ports::audio::AudioDevice;

use super::args::DevicesArgs;

/// Run the `devices` subcommand.
pub fn run(args: &DevicesArgs) -> Result<()> {
    let devices = coreaudio::enumerate()?;
    if args.json {
        let items: Vec<serde_json::Value> = devices.iter().map(device_json).collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        println!("Output devices:");
        print_direction(&devices, false);
        println!("Input devices:");
        print_direction(&devices, true);
    }
    Ok(())
}

/// Render one device as a JSON object (FR-10 fields).
fn device_json(d: &AudioDevice) -> serde_json::Value {
    serde_json::json!({
        "id": d.id.0,
        "uid": d.uid,
        "name": d.name,
        "input_channels": d.input_channels,
        "output_channels": d.output_channels,
        "sample_rate": d.sample_rate,
        "default_input": d.is_default_input,
        "default_output": d.is_default_output,
    })
}

/// Print every device participating in the requested direction.
fn print_direction(devices: &[AudioDevice], input: bool) {
    let mut shown = false;
    for d in devices.iter().filter(|d| directional(d, input)) {
        shown = true;
        println!("{}", device_line(d, input));
    }
    if !shown {
        println!("  (none)");
    }
}

/// Whether `d` participates in the requested direction (input vs output).
fn directional(d: &AudioDevice, input: bool) -> bool {
    if input { d.is_input() } else { d.is_output() }
}

/// One device table row (`* [id] name  Nch @ rate Hz  uid=...`).
fn device_line(d: &AudioDevice, input: bool) -> String {
    let (default, channels) = if input {
        (d.is_default_input, d.input_channels)
    } else {
        (d.is_default_output, d.output_channels)
    };
    let mark = if default { '*' } else { ' ' };
    format!(
        "{mark} [{id:>3}] {name:<28} {channels:>2}ch @ {rate:>5} Hz  uid={uid}",
        id = d.id.0,
        name = d.name,
        rate = d.sample_rate,
        uid = d.uid,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use speak::ports::audio::AudioDeviceId;

    fn sample_device() -> AudioDevice {
        AudioDevice {
            id: AudioDeviceId(7),
            uid: "UID7".into(),
            name: "Speakers".into(),
            input_channels: 0,
            output_channels: 2,
            sample_rate: 48_000,
            is_default_input: false,
            is_default_output: true,
        }
    }

    #[test]
    fn device_line_marks_default_and_shows_id_uid_channels() {
        let line = device_line(&sample_device(), false);
        assert!(
            line.starts_with('*'),
            "default output should be starred: {line}"
        );
        assert!(line.contains("[  7]"), "{line}");
        assert!(line.contains("uid=UID7"), "{line}");
        assert!(line.contains("2ch"), "{line}");
        assert!(line.contains("48000 Hz"), "{line}");
    }

    #[test]
    fn device_line_input_uses_input_channels_and_default() {
        let line = device_line(&sample_device(), true);
        assert!(line.starts_with(' '), "{line}");
        assert!(line.contains(" 0ch"), "{line}");
    }

    #[test]
    fn device_json_exposes_fr10_fields() {
        let v = device_json(&sample_device());
        assert_eq!(v["id"], 7);
        assert_eq!(v["uid"], "UID7");
        assert_eq!(v["output_channels"], 2);
        assert_eq!(v["sample_rate"], 48_000);
        assert_eq!(v["default_output"], true);
        assert_eq!(v["default_input"], false);
    }
}
