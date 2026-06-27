//! `Presenter` driven port (ADR-0009).
//!
//! The single seam every command RESULT flows through, so no use case or
//! driving adapter scatters raw `println!`. It carries structured payloads —
//! key/value [`Report`]s (`check`, `config show`, `say --json` metadata),
//! [`Table`]s (`devices`, `voices list`, `list-designs`), and free-form result
//! [`line`](Presenter::line)s (a transcript, a realtime caption) — so a swappable
//! adapter can render them as coloured human text, machine-readable JSON, or a
//! captured test buffer (console | json | test buffer). The `--quiet`, `--json`,
//! and `--color`/`NO_COLOR` behaviour is the adapter's concern, not the port's;
//! DIAGNOSTICS never come here — they go to `tracing` (stderr + rotating file).
//! No framework type crosses this boundary.

use anyhow::Result;

/// A key/value result report rendered as a titled block (or a JSON object).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    title: Option<String>,
    entries: Vec<(String, String)>,
}

impl Report {
    /// Start an untitled report.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a report under `title` (fluent).
    #[must_use]
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            entries: Vec::new(),
        }
    }

    /// Append a `key = value` entry (fluent).
    #[must_use]
    pub fn entry(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push((key.into(), value.into()));
        self
    }

    /// The optional report title.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The ordered key/value entries.
    #[must_use]
    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }

    /// Whether the report carries no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A tabular result rendered as aligned columns (or a JSON array of rows).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// Start a table with the given column `headers` (fluent).
    #[must_use]
    pub fn new<I, S>(headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    /// Append a row of `cells` (fluent).
    #[must_use]
    pub fn row<I, S>(mut self, cells: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rows.push(cells.into_iter().map(Into::into).collect());
        self
    }

    /// The column headers.
    #[must_use]
    pub fn headers(&self) -> &[String] {
        &self.headers
    }

    /// The data rows.
    #[must_use]
    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    /// The number of columns (header count).
    #[must_use]
    pub fn column_count(&self) -> usize {
        self.headers.len()
    }
}

/// Driven port: emit command RESULTS through a swappable presenter.
///
/// Each method renders one structured result; the adapter decides the concrete
/// form (human text, JSON, or a capture buffer) and honours `--quiet`/`--json`/
/// `--color`. Diagnostics do NOT flow here.
pub trait Presenter {
    /// Emit a key/value [`Report`].
    fn report(&mut self, report: &Report) -> Result<()>;

    /// Emit a [`Table`].
    fn table(&mut self, table: &Table) -> Result<()>;

    /// Emit a single free-form result line.
    fn line(&mut self, text: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    /// A capture-buffer presenter proving the port is unit-testable (ADR-0009).
    #[derive(Default)]
    struct BufferPresenter {
        out: String,
    }

    impl Presenter for BufferPresenter {
        fn report(&mut self, report: &Report) -> Result<()> {
            if let Some(title) = report.title() {
                writeln!(self.out, "[{title}]")?;
            }
            for (key, value) in report.entries() {
                writeln!(self.out, "{key} = {value}")?;
            }
            Ok(())
        }

        fn table(&mut self, table: &Table) -> Result<()> {
            writeln!(self.out, "{}", table.headers().join("\t"))?;
            for row in table.rows() {
                writeln!(self.out, "{}", row.join("\t"))?;
            }
            Ok(())
        }

        fn line(&mut self, text: &str) -> Result<()> {
            writeln!(self.out, "{text}")?;
            Ok(())
        }
    }

    #[test]
    fn report_builder_preserves_title_and_order() {
        let report = Report::titled("health")
            .entry("status", "healthy")
            .entry("models", "5");
        assert_eq!(report.title(), Some("health"));
        assert_eq!(report.entries().len(), 2);
        assert_eq!(report.entries()[0], ("status".into(), "healthy".into()));
        assert!(!report.is_empty());
        assert!(Report::new().is_empty());
    }

    #[test]
    fn table_builder_collects_headers_and_rows() {
        let table = Table::new(["id", "name"])
            .row(["1", "alloy"])
            .row(["2", "narrator"]);
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.rows().len(), 2);
        assert_eq!(table.rows()[1], vec!["2".to_owned(), "narrator".to_owned()]);
    }

    #[test]
    fn presenter_renders_through_the_port() {
        let mut presenter = BufferPresenter::default();
        presenter
            .report(&Report::titled("check").entry("host", "solaris"))
            .unwrap();
        presenter
            .table(&Table::new(["device"]).row(["Speakers"]))
            .unwrap();
        presenter.line("hello world").unwrap();
        assert_eq!(
            presenter.out,
            "[check]\nhost = solaris\ndevice\nSpeakers\nhello world\n"
        );
    }

    #[test]
    fn presenter_is_object_safe() {
        // Confirms the composition root can inject it as a boxed trait object.
        let mut presenter: Box<dyn Presenter> = Box::new(BufferPresenter::default());
        presenter.line("via dyn").unwrap();
    }
}
