//! Turning displays into something to read, or something to pipe.

use klart_core::{Control, DisplayKind};
use serde_json::{Value, json};

/// The table `list`, `get`, `set`, `up` and `down` print.
///
/// `verbose` adds the columns only `list` has room to justify — the key, which
/// is long, and the reasons the better mechanisms declined.
pub fn table(controls: &[&Control], indices: &[usize], verbose: bool) {
    let rows: Vec<Row> = controls
        .iter()
        .zip(indices)
        .map(|(control, &index)| Row::of(control, index))
        .collect();

    if rows.is_empty() {
        return;
    }

    let name_width = rows.iter().map(|row| row.name.len()).max().unwrap_or(0);
    let mechanism_width = rows
        .iter()
        .map(|row| row.mechanism.len())
        .max()
        .unwrap_or(0);

    if verbose {
        // Only `list` gets a header. `get` is the one people pipe into other
        // things, and a header there would be a line to strip.
        println!(
            "{index:>3}  {level:>5}  {mechanism:<mechanism_width$}  {name:<name_width$}  KEY",
            index = "IDX",
            level = "LEVEL",
            mechanism = "MECHANISM",
            name = "DISPLAY",
        );
    }

    for row in &rows {
        println!(
            "{index:>3}  {level:>5}  {mechanism:<mechanism_width$}  {name:<name_width$}{key}",
            index = row.index,
            level = row.level,
            mechanism = row.mechanism,
            name = row.name,
            key = if verbose {
                format!("  {}", row.key)
            } else {
                String::new()
            },
        );

        if verbose {
            for refusal in &row.refusals {
                println!("       {refusal}");
            }
        }
    }
}

/// Says plainly when a change has already been undone.
///
/// macOS reverts a gamma ramp when the process that set it exits, so a `set` on
/// a display with no hardware mechanism has done nothing by the time this
/// process returns. Printing the new level and stopping there would be a lie.
pub fn warn_about_anything_that_will_not_last(controls: &[&Control]) {
    for control in controls {
        if !control.persists() {
            eprintln!(
                "klart: part of {}'s level is being held by its gamma ramp, which macOS puts \
                 back as this command exits — so the change is already gone. Only a process \
                 that keeps running can hold it; `klart-tray` is that process.",
                control.name()
            );
        }
    }
}

/// The same displays, for something other than a person.
pub fn json(controls: &[&Control], indices: &[usize]) {
    let displays: Vec<Value> = controls
        .iter()
        .zip(indices)
        .map(|(control, &index)| {
            let display = control.display();
            let bounds = display.bounds();

            json!({
                "index": index,
                "id": display.id(),
                "name": control.name(),
                "key": display.key().as_str(),
                "kind": match display.kind() {
                    DisplayKind::BuiltIn => "built-in",
                    DisplayKind::External => "external",
                },
                "main": display.is_main(),
                "bounds": {
                    "x": bounds.x,
                    "y": bounds.y,
                    "width": bounds.width,
                    "height": bounds.height,
                },
                "percent": control.get().ok().map(|level| level.percent_rounded()),
                "mechanism": control.mechanism(),
                "persists": control.persists(),
                "refusals": control
                    .refusals()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string_pretty(&displays).unwrap_or_else(|_| "[]".to_owned())
    );
}

struct Row {
    index: usize,
    level: String,
    mechanism: String,
    name: String,
    key: String,
    refusals: Vec<String>,
}

impl Row {
    fn of(control: &Control, index: usize) -> Self {
        Self {
            index,
            level: match control.get() {
                Ok(level) => level.to_string(),
                // A display that will not answer still belongs in the list; the
                // reason it did not is on the line below when `list` asked.
                Err(_) => "?".to_owned(),
            },
            mechanism: match (control.mechanism(), control.persists()) {
                (Some(name), true) => name.to_owned(),
                (Some(name), false) => format!("{name}*"),
                // Nothing reaches this display. The reasons print underneath
                // when `list` asked for them.
                (None, _) => "none".to_owned(),
            },
            name: format!(
                "{}{}",
                control.name(),
                if control.display().is_main() {
                    " (main)"
                } else {
                    ""
                }
            ),
            key: control.display().key().to_string(),
            refusals: control.refusals().iter().map(ToString::to_string).collect(),
        }
    }
}

/// What `probe` prints.
///
/// Laid out as evidence and then a conclusion, rather than the other way round,
/// because the conclusion is a guess built from the evidence and a reader who
/// disagrees with it needs to be able to see why.
pub fn probe(reports: &[klart_core::Report]) {
    if reports.is_empty() {
        println!("no displays are attached");
        return;
    }

    for (index, report) in reports.iter().enumerate() {
        if index > 0 {
            println!();
        }

        println!("{} ({})", report.display, report.key);

        for note in &report.notes {
            println!("  {:<16} {}", note.label, note.value);
        }

        for attempt in &report.attempts {
            match &attempt.outcome {
                Ok(summary) => println!("  ok               {}: {summary}", attempt.what),
                Err(problem) => println!("  failed           {}: {problem}", attempt.what),
            }
        }

        println!();
        println!("  {}", report.verdict.name());
        for line in wrap(report.verdict.advice(), 72) {
            println!("  {line}");
        }
    }

    corroborate(reports);
}

/// What `probe --json` prints.
///
/// An object rather than the bare array `list --json` emits, and deliberately:
/// `list` answers "what is attached", which is a list, while `probe` answers
/// "what was found", which has a conclusion above the per-display detail. The
/// corroboration is that conclusion and has nowhere to live in an array.
///
/// It carries everything the text form does. A probe transcript exists to be
/// handed to somebody else, and a machine-readable one that dropped half the
/// evidence would be worse than pasting the text.
pub fn probe_json(reports: &[klart_core::Report]) {
    println!(
        "{}",
        serde_json::to_string_pretty(&probe_document(reports)).unwrap_or_else(|_| "{}".to_owned())
    );
}

/// The document [`probe_json`] prints.
///
/// Built apart from the printing so that the shape — which is a published
/// interface the moment anyone parses it — can be asserted in a test rather than
/// eyeballed in a terminal.
fn probe_document(reports: &[klart_core::Report]) -> Value {
    let displays: Vec<Value> = reports
        .iter()
        .map(|report| {
            json!({
                "display": report.display,
                "key": report.key,
                "kind": match report.kind {
                    DisplayKind::BuiltIn => "built-in",
                    DisplayKind::External => "external",
                },
                "notes": report
                    .notes
                    .iter()
                    .map(|note| json!({ "label": note.label, "value": note.value }))
                    .collect::<Vec<_>>(),
                "attempts": report
                    .attempts
                    .iter()
                    .map(|attempt| {
                        // The same keys whether it worked or not. A consumer
                        // filtering on `ok` should not also have to know which
                        // of two field names to reach for.
                        let (ok, detail) = match &attempt.outcome {
                            Ok(summary) => (true, summary),
                            Err(problem) => (false, problem),
                        };
                        json!({ "what": attempt.what, "ok": ok, "detail": detail })
                    })
                    .collect::<Vec<_>>(),
                "address_honoured": report.address_honoured,
                "verdict": report.verdict.name(),
                "advice": report.verdict.advice(),
            })
        })
        .collect();

    let corroborated = corroboration(reports)
        .map(|(honoured, ignored)| json!({ "honoured": honoured, "ignored": ignored }));

    json!({
        "displays": displays,
        // Null rather than absent, so a consumer can read the key
        // unconditionally instead of testing for it.
        "corroborated": corroborated,
    })
}

/// The displays that honoured the I2C chip address and those that ignored it,
/// when there is at least one of each.
///
/// [`None`] when the comparison cannot be made, which is the ordinary case on a
/// machine with one display. Shared by both renderers rather than derived twice:
/// this is the finding the whole probe turns on, and the text and the JSON
/// disagreeing about it would be worse than either being wrong alone.
fn corroboration(reports: &[klart_core::Report]) -> Option<(Vec<&str>, Vec<&str>)> {
    let honoured: Vec<&str> = reports
        .iter()
        .filter(|report| report.address_honoured == Some(true))
        .map(|report| report.display.as_str())
        .collect();
    let ignored: Vec<&str> = reports
        .iter()
        .filter(|report| report.address_honoured == Some(false))
        .map(|report| report.display.as_str())
        .collect();

    if honoured.is_empty() || ignored.is_empty() {
        return None;
    }
    Some((honoured, ignored))
}

/// Says so when the displays disagree about whether the chip address is
/// honoured.
///
/// One link ignoring it proves only that something is wrong. Another link on the
/// same machine honouring it, through the same calls in the same process, is
/// what proves the difference is the link rather than the interface.
fn corroborate(reports: &[klart_core::Report]) {
    let Some((honoured, ignored)) = corroboration(reports) else {
        return;
    };

    println!();
    for line in wrap(
        &format!(
            "Corroborated on this machine: {} honoured the I2C chip address and {} ignored it, \
             through the same calls in the same process. The difference is the link, not the \
             interface.",
            honoured.join(", "),
            ignored.join(", ")
        ),
        74,
    ) {
        println!("{line}");
    }
}

/// Breaks a paragraph so a terminal does not have to.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();

    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use klart_core::{Attempt, Note, Report, Verdict};

    fn report(display: &str, honoured: Option<bool>, verdict: Verdict) -> Report {
        Report {
            display: display.to_owned(),
            key: format!("{display}-key"),
            kind: DisplayKind::External,
            notes: vec![Note {
                label: "link".to_owned(),
                value: "DP -> HDMI".to_owned(),
            }],
            attempts: vec![
                Attempt {
                    what: "read the EDID".to_owned(),
                    outcome: Ok("valid header".to_owned()),
                },
                Attempt {
                    what: "Get VCP 0x10".to_owned(),
                    outcome: Err("0xe0114102".to_owned()),
                },
            ],
            address_honoured: honoured,
            verdict,
        }
    }

    /// The finding both renderers read.
    ///
    /// One link ignoring the chip address proves only that something is wrong.
    /// Another honouring it, in the same process, is what proves the difference
    /// is the link — so the comparison needs one of each and nothing less will
    /// do.
    #[test]
    fn corroboration_needs_a_display_on_each_side() {
        let both = [
            report("ignores", Some(false), Verdict::EdidOnly),
            report("honours", Some(true), Verdict::Answers),
        ];
        let (honoured, ignored) = corroboration(&both).expect("one of each corroborates");
        assert_eq!(honoured, ["honours"]);
        assert_eq!(ignored, ["ignores"]);

        // The ordinary case on a laptop with one monitor: nothing to compare
        // against, so there is no finding to report rather than a weak one.
        let alone = [report("ignores", Some(false), Verdict::EdidOnly)];
        assert!(corroboration(&alone).is_none());

        let agreeing = [
            report("a", Some(true), Verdict::Answers),
            report("b", Some(true), Verdict::Answers),
        ];
        assert!(corroboration(&agreeing).is_none());
    }

    /// The shape is an interface the moment anybody parses it.
    #[test]
    fn the_document_carries_what_the_transcript_does() {
        let reports = [
            report("ignores", Some(false), Verdict::EdidOnly),
            report("honours", Some(true), Verdict::Answers),
        ];
        let document = probe_document(&reports);

        assert_eq!(
            document["corroborated"]["honoured"][0], "honours",
            "the finding the whole probe turns on has to be in the document"
        );
        assert_eq!(document["corroborated"]["ignored"][0], "ignores");

        let first = &document["displays"][0];
        assert_eq!(first["display"], "ignores");
        assert_eq!(first["key"], "ignores-key");
        assert_eq!(
            first["kind"], "external",
            "the same word `list --json` uses"
        );
        assert_eq!(first["address_honoured"], false);
        assert_eq!(
            first["verdict"], "EdidOnly",
            "the published name, not the Debug spelling of the day"
        );
        assert!(
            first["advice"].as_str().is_some_and(|a| !a.is_empty()),
            "a verdict with no advice is the failure this tool exists to avoid"
        );
        assert_eq!(first["notes"][0]["label"], "link");
        assert_eq!(first["notes"][0]["value"], "DP -> HDMI");
    }

    /// Both outcomes carry the same keys.
    ///
    /// A consumer filtering on `ok` should not also have to know which of two
    /// field names to reach for depending on the answer.
    #[test]
    fn an_attempt_has_the_same_keys_whether_it_worked_or_not() {
        let reports = [report("d", Some(true), Verdict::Answers)];
        let attempts = probe_document(&reports)["displays"][0]["attempts"].clone();

        let succeeded = &attempts[0];
        let failed = &attempts[1];

        assert_eq!(succeeded["ok"], true);
        assert_eq!(succeeded["detail"], "valid header");
        assert_eq!(failed["ok"], false);
        assert_eq!(failed["detail"], "0xe0114102");

        for attempt in [succeeded, failed] {
            let mut keys: Vec<&str> = attempt
                .as_object()
                .expect("an attempt is an object")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, ["detail", "ok", "what"]);
        }
    }

    /// The ordinary case, and the one this machine cannot produce.
    #[test]
    fn corroborated_is_null_rather_than_missing_when_there_is_no_finding() {
        let alone = [report("only one", Some(false), Verdict::EdidOnly)];
        let document = probe_document(&alone);

        assert!(
            document
                .as_object()
                .is_some_and(|d| d.contains_key("corroborated")),
            "the key has to be there so a consumer can read it unconditionally"
        );
        assert!(document["corroborated"].is_null());
    }

    #[test]
    fn no_displays_is_an_empty_list_rather_than_an_error() {
        let document = probe_document(&[]);
        assert_eq!(document["displays"].as_array().map(Vec::len), Some(0));
        assert!(document["corroborated"].is_null());
    }

    /// A display that was never asked cannot take a side.
    #[test]
    fn a_display_with_no_answer_is_on_neither_side() {
        let reports = [
            report("unknown", None, Verdict::NoChannel),
            report("honours", Some(true), Verdict::Answers),
        ];
        assert!(
            corroboration(&reports).is_none(),
            "`None` must not be counted as ignoring the address"
        );
    }
}
