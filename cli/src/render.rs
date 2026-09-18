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
        println!("  {:?}", report.verdict);
        for line in wrap(report.verdict.advice(), 72) {
            println!("  {line}");
        }
    }

    corroborate(reports);
}

/// Says so when the displays disagree about whether the chip address is
/// honoured.
///
/// One link ignoring it proves only that something is wrong. Another link on the
/// same machine honouring it, through the same calls in the same process, is
/// what proves the difference is the link rather than the interface.
fn corroborate(reports: &[klart_core::Report]) {
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
        return;
    }

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
