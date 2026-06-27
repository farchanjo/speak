//! `presenter` driven adapters (T048 / ADR-0009): the swappable renderers behind
//! the [`crate::ports::presenter::Presenter`] port.
//!
//! The composition root selects one **Strategy** from the global flags and
//! injects it as a `Box<dyn Presenter>`: [`console::ConsolePresenter`] for
//! coloured human text (honouring `--quiet` and `--color`/`NO_COLOR`) or
//! [`json::JsonPresenter`] for machine-readable output (`--json`, FR-16). Command
//! RESULTS flow through the port; DIAGNOSTICS ride `tracing` (ADR-0002).

pub mod console;
pub mod json;

use std::io::{self, IsTerminal};

use crate::ports::presenter::Presenter;

pub use console::ConsolePresenter;
pub use json::JsonPresenter;

/// Build the presenter the composition root injects into every handler.
///
/// `json` selects the machine-readable renderer (FR-16); otherwise the console
/// renderer is used with the resolved `quiet`/`color` behaviour.
#[must_use]
pub fn build(json: bool, quiet: bool, color: bool) -> Box<dyn Presenter> {
    if json {
        Box::new(JsonPresenter::new(io::stdout()))
    } else {
        Box::new(ConsolePresenter::new(io::stdout(), color, quiet))
    }
}

/// Resolve whether ANSI colour should be emitted: the configured `[general].color`
/// preference, suppressed by the `NO_COLOR` convention or a non-terminal stdout.
#[must_use]
pub fn color_enabled(config_color: bool) -> bool {
    config_color && std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::presenter::Report;

    #[test]
    fn build_returns_an_object_safe_presenter() {
        // Both arms satisfy `dyn Presenter` (object-safety regression guard).
        let mut p = build(true, false, false);
        p.report(&Report::new().entry("k", "v")).unwrap();
        let mut c = build(false, true, true);
        c.line("ok").unwrap();
    }

    #[test]
    fn color_is_disabled_when_no_color_is_set() {
        // SAFETY: env mutation serialised on the process-wide test lock.
        let _guard = crate::testenv::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("NO_COLOR");
        unsafe { std::env::set_var("NO_COLOR", "1") };
        assert!(!color_enabled(true));
        match prev {
            Some(v) => unsafe { std::env::set_var("NO_COLOR", v) },
            None => unsafe { std::env::remove_var("NO_COLOR") },
        }
    }
}
