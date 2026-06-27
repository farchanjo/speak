//! Console `Presenter` adapter (T048 / ADR-0009): coloured, aligned human text.
//!
//! Renders each command RESULT to a writer (stdout in production). It honours
//! `--color`/`NO_COLOR` (ANSI only when colour is enabled) and `--quiet`: a
//! quiet console suppresses the decorated `report`/`table` inventory views while
//! still emitting raw `line` results, so a transcript stays pipeable under `-q`.
//! Diagnostics never reach this adapter — they ride `tracing` (ADR-0002).

use std::io::Write;

use anyhow::Result;

use crate::ports::presenter::{Presenter, Report, Table};

/// ANSI reset.
const RESET: &str = "\x1b[0m";
/// ANSI bold (report titles / table headers).
const BOLD: &str = "\x1b[1m";

/// A `Presenter` that writes aligned, optionally coloured text to `out`.
pub struct ConsolePresenter<W: Write> {
    out: W,
    color: bool,
    quiet: bool,
}

impl<W: Write> ConsolePresenter<W> {
    /// Build a console presenter over `out`.
    #[must_use]
    pub fn new(out: W, color: bool, quiet: bool) -> Self {
        Self { out, color, quiet }
    }

    /// Wrap `text` in `code`..reset when colour is enabled, else pass through.
    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("{code}{text}{RESET}")
        } else {
            text.to_owned()
        }
    }

    /// The display width of the longest key, for `report` alignment.
    fn key_width(entries: &[(String, String)]) -> usize {
        entries.iter().map(|(k, _)| k.len()).max().unwrap_or(0)
    }

    /// Per-column widths spanning the headers and every row of `table`.
    fn column_widths(table: &Table) -> Vec<usize> {
        let mut widths: Vec<usize> = table.headers().iter().map(String::len).collect();
        for row in table.rows() {
            for (i, cell) in row.iter().enumerate() {
                if let Some(w) = widths.get_mut(i) {
                    *w = (*w).max(cell.len());
                }
            }
        }
        widths
    }
}

impl<W: Write> Presenter for ConsolePresenter<W> {
    fn report(&mut self, report: &Report) -> Result<()> {
        if self.quiet {
            return Ok(());
        }
        if let Some(title) = report.title() {
            writeln!(self.out, "{}", self.paint(BOLD, title))?;
        }
        let width = Self::key_width(report.entries());
        for (key, value) in report.entries() {
            writeln!(self.out, "{key:<width$}  {value}")?;
        }
        Ok(())
    }

    fn table(&mut self, table: &Table) -> Result<()> {
        if self.quiet {
            return Ok(());
        }
        let widths = Self::column_widths(table);
        if !table.headers().is_empty() {
            let header = render_row(table.headers(), &widths);
            writeln!(self.out, "{}", self.paint(BOLD, &header))?;
        }
        for row in table.rows() {
            writeln!(self.out, "{}", render_row(row, &widths))?;
        }
        Ok(())
    }

    fn line(&mut self, text: &str) -> Result<()> {
        writeln!(self.out, "{text}")?;
        Ok(())
    }
}

/// Render one space-padded table row to the per-column `widths`.
fn render_row(cells: &[String], widths: &[usize]) -> String {
    cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            format!(
                "{cell:<width$}",
                width = widths.get(i).copied().unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render<F>(color: bool, quiet: bool, body: F) -> String
    where
        F: FnOnce(&mut ConsolePresenter<&mut Vec<u8>>) -> Result<()>,
    {
        let mut buf = Vec::new();
        {
            let mut p = ConsolePresenter::new(&mut buf, color, quiet);
            body(&mut p).unwrap();
        }
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn report_aligns_keys_and_paints_title_when_colour() {
        let report = Report::titled("health")
            .entry("status", "ok")
            .entry("n", "5");
        let out = render(true, false, |p| p.report(&report));
        assert!(out.contains(BOLD), "title is painted when colour on");
        assert!(out.contains("status  ok"));
        assert!(out.contains("n       5"), "keys aligned to width: {out:?}");
    }

    #[test]
    fn no_colour_omits_ansi() {
        let report = Report::titled("health").entry("status", "ok");
        let out = render(false, false, |p| p.report(&report));
        assert!(!out.contains('\x1b'), "no ANSI when colour off: {out:?}");
        assert!(out.starts_with("health\n"));
    }

    #[test]
    fn quiet_suppresses_report_and_table_but_not_line() {
        let report = Report::titled("t").entry("a", "b");
        assert_eq!(render(false, true, |p| p.report(&report)), "");
        let table = Table::new(["h"]).row(["x"]);
        assert_eq!(render(false, true, |p| p.table(&table)), "");
        assert_eq!(render(false, true, |p| p.line("piped")), "piped\n");
    }

    #[test]
    fn table_aligns_columns() {
        let table = Table::new(["id", "name"])
            .row(["1", "alloy"])
            .row(["20", "x"]);
        let out = render(false, false, |p| p.table(&table));
        assert!(out.contains("id  name"));
        assert!(out.contains("1   alloy"));
        assert!(out.contains("20  x"));
    }
}
