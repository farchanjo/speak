//! JSON `Presenter` adapter (T048 / ADR-0009 / FR-16): machine-readable output.
//!
//! Renders each command RESULT as one JSON document per call (newline-delimited
//! when a command emits several). A [`Report`] becomes an object of its entries
//! (with an optional `title`), a [`Table`] an array of header-keyed row objects,
//! and a `line` a JSON string. JSON is for piping, so `--quiet` does not gag it.
//! The serialisation lives here, never on the port — no `serde_json` type crosses
//! the [`Presenter`] boundary.

use std::io::Write;

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::ports::presenter::{Presenter, Report, Table};

/// A `Presenter` that serialises each result as JSON to `out`.
pub struct JsonPresenter<W: Write> {
    out: W,
}

impl<W: Write> JsonPresenter<W> {
    /// Build a JSON presenter over `out`.
    #[must_use]
    pub fn new(out: W) -> Self {
        Self { out }
    }

    /// Write one JSON document followed by a newline.
    fn emit(&mut self, value: &Value) -> Result<()> {
        writeln!(self.out, "{}", serde_json::to_string(value)?)?;
        Ok(())
    }
}

impl<W: Write> Presenter for JsonPresenter<W> {
    fn report(&mut self, report: &Report) -> Result<()> {
        let entries: Map<String, Value> = report
            .entries()
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let value = match report.title() {
            Some(title) => json!({ "title": title, "entries": entries }),
            None => Value::Object(entries),
        };
        self.emit(&value)
    }

    fn table(&mut self, table: &Table) -> Result<()> {
        let headers = table.headers();
        let rows: Vec<Value> = table
            .rows()
            .iter()
            .map(|row| {
                let obj: Map<String, Value> = headers
                    .iter()
                    .zip(row.iter())
                    .map(|(h, c)| (h.clone(), Value::String(c.clone())))
                    .collect();
                Value::Object(obj)
            })
            .collect();
        self.emit(&Value::Array(rows))
    }

    fn line(&mut self, text: &str) -> Result<()> {
        self.emit(&Value::String(text.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render<F>(body: F) -> String
    where
        F: FnOnce(&mut JsonPresenter<&mut Vec<u8>>) -> Result<()>,
    {
        let mut buf = Vec::new();
        {
            let mut p = JsonPresenter::new(&mut buf);
            body(&mut p).unwrap();
        }
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn titled_report_carries_title_and_entries() {
        let report = Report::titled("synthesis").entry("rtf", "0.42");
        let out = render(|p| p.report(&report));
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["title"], "synthesis");
        assert_eq!(v["entries"]["rtf"], "0.42");
    }

    #[test]
    fn untitled_report_is_a_flat_object() {
        let report = Report::new().entry("host", "solaris");
        let out = render(|p| p.report(&report));
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["host"], "solaris");
    }

    #[test]
    fn table_is_array_of_row_objects() {
        let table = Table::new(["id", "name"]).row(["1", "alloy"]);
        let out = render(|p| p.table(&table));
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v[0]["id"], "1");
        assert_eq!(v[0]["name"], "alloy");
    }

    #[test]
    fn line_is_a_json_string() {
        let out = render(|p| p.line("olá"));
        assert_eq!(out.trim(), "\"olá\"");
    }
}
