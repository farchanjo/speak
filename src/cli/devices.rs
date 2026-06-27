//! `devices` handler (T056): enumerate input/output audio devices (FR-10).
//!
//! A thin CLI adapter that reads the `coreaudio` device-enumeration adapter
//! directly (no dedicated use case, per ADR-0003) and emits a single device
//! Table through the [`Presenter`] port — one row per device with its
//! `AudioDeviceID`, name, channels, rate, default flags, and UID. The console
//! renderer aligns it; the json renderer serialises it (FR-16 / ADR-0009).

use anyhow::Result;

use speak::adapters::coreaudio;
use speak::ports::audio::AudioDevice;
use speak::ports::presenter::{Presenter, Table};

/// Run the `devices` subcommand, emitting the device inventory through the
/// Presenter.
pub fn run(presenter: &mut dyn Presenter) -> Result<()> {
    let devices = coreaudio::enumerate()?;
    let mut table = Table::new([
        "id",
        "name",
        "in_ch",
        "out_ch",
        "rate_hz",
        "default_in",
        "default_out",
        "uid",
    ]);
    for device in &devices {
        table = table.row(device_row(device));
    }
    presenter.table(&table)
}

/// Project one device onto its Table row (FR-10 fields).
fn device_row(d: &AudioDevice) -> [String; 8] {
    [
        d.id.0.to_string(),
        d.name.clone(),
        d.input_channels.to_string(),
        d.output_channels.to_string(),
        d.sample_rate.to_string(),
        yes_no(d.is_default_input),
        yes_no(d.is_default_output),
        d.uid.clone(),
    ]
}

/// Render a boolean default-device flag.
fn yes_no(flag: bool) -> String {
    if flag { "yes" } else { "no" }.to_owned()
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
    fn device_row_exposes_fr10_fields() {
        let row = device_row(&sample_device());
        assert_eq!(
            row,
            [
                "7".to_owned(),
                "Speakers".to_owned(),
                "0".to_owned(),
                "2".to_owned(),
                "48000".to_owned(),
                "no".to_owned(),
                "yes".to_owned(),
                "UID7".to_owned(),
            ]
        );
    }
}
