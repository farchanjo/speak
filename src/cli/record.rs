//! `record` handler (T055): capture the microphone to a WAV/FLAC file (FR-9).
//!
//! A thin driving adapter: it maps the flags to a [`RecordOptions`] value object,
//! runs the application record use case (via the [`AppFacade`]), writes the muxed
//! bytes to `--output` (a driving-adapter concern), and reports the result through
//! the [`Presenter`]. Capture and encoding stay behind the ports.

use anyhow::{Context, Result};

use speak::adapters::config::Config;
use speak::application::RecordOptions;
use speak::ports::audio::AudioDeviceId;
use speak::ports::presenter::{Presenter, Report};

use super::AppFacade;
use super::args::RecordArgs;

/// Run the `record` subcommand.
pub(crate) async fn run(
    facade: &AppFacade,
    cfg: &Config,
    args: RecordArgs,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let format = args.format.to_record_format();
    let opts = RecordOptions {
        device: args.device.map(AudioDeviceId),
        secs: args.duration,
        format,
        sample_rate: args.sample_rate,
        channels: args.channels,
        input_channel: args.input_channel.or(cfg.audio.input.channel),
    };
    let outcome = facade.record(&opts).await?;
    tokio::fs::write(&args.output, &outcome.bytes)
        .await
        .with_context(|| format!("writing {}", args.output.display()))?;
    let report = Report::titled("record")
        .entry("file", args.output.display().to_string())
        .entry("format", format!("{:?}", outcome.format).to_lowercase())
        .entry("frames", outcome.frames.to_string())
        .entry("seconds", format!("{:.2}", outcome.secs))
        .entry("bytes", outcome.bytes.len().to_string());
    presenter.report(&report)
}
