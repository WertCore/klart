//! `klart` on the command line.

mod render;
mod select;

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use klart_core::{Brightness, Control, LoginItem, Remembered, controls};

use crate::select::{Candidate, Selection};

/// The step `up` and `down` take when not told otherwise.
const DEFAULT_STEP: f32 = 10.0;

#[derive(Parser)]
#[command(
    name = "klart",
    version,
    about = "Brightness for every display attached to the machine",
    long_about = "Brightness for every display attached to the machine.\n\n\
        Commands act on the main display, which on macOS is the one with the menu \
        bar, unless told otherwise. \
        `--display` takes an index, a key or part of a name, all as printed by \
        `klart list`."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show every attached display, its level and how klart reaches it.
    List {
        #[command(flatten)]
        output: Output,
    },
    /// Print a display's level.
    Get {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        output: Output,
    },
    /// Set a display's level, as a percentage.
    Set {
        /// A percentage, with or without the sign: `40` and `40%` are the same.
        percent: Percent,
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        output: Output,
    },
    /// Make a display brighter.
    Up {
        /// How far, in percentage points.
        #[arg(default_value_t = DEFAULT_STEP)]
        step: f32,
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        output: Output,
    },
    /// Work out why a display will not answer, and what to do about it.
    Probe {
        #[command(flatten)]
        output: Output,
    },
    /// Call a display something other than what it calls itself.
    ///
    /// Two monitors of the same model publish the same name, which is the case
    /// this exists for.
    Rename {
        /// Which display: an index, a key, or part of a name.
        #[arg(value_name = "INDEX|KEY|NAME")]
        display: String,
        /// The new name. Omit it to give the display its own name back.
        name: Option<String>,
    },
    /// Show or change whether the menu bar agent starts with the session.
    ///
    /// Worth setting: on a display with no hardware brightness control the level
    /// lasts only as long as the agent runs, so without this such a display is
    /// back at full brightness after every restart.
    Autostart {
        /// `on` or `off`. Omit to print the current setting.
        #[arg(value_name = "on|off")]
        wanted: Option<Switch>,
    },
    /// Put every display back to the level klart last saw it at.
    Restore {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        output: Output,
    },
    /// Make a display dimmer.
    Down {
        /// How far, in percentage points.
        #[arg(default_value_t = DEFAULT_STEP)]
        step: f32,
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        output: Output,
    },
}

#[derive(Args)]
struct Target {
    /// Which display: an index, a key, or part of a name.
    #[arg(short, long, value_name = "INDEX|KEY|NAME")]
    display: Option<String>,

    /// Every attached display.
    #[arg(short, long, conflicts_with = "display")]
    all: bool,
}

impl Target {
    fn selection(&self) -> Selection {
        match (&self.display, self.all) {
            (Some(wanted), _) => Selection::Named(wanted.clone()),
            (None, true) => Selection::All,
            (None, false) => Selection::Main,
        }
    }
}

#[derive(Args)]
struct Output {
    /// Print machine-readable output instead of a table.
    #[arg(long)]
    json: bool,
}

/// A plain on or off.
#[derive(Clone, Copy)]
struct Switch(bool);

impl std::str::FromStr for Switch {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "enable" | "enabled" => Ok(Self(true)),
            "off" | "false" | "no" | "disable" | "disabled" => Ok(Self(false)),
            other => Err(format!("{other:?} is not `on` or `off`")),
        }
    }
}

/// A percentage, accepted with or without a trailing sign.
#[derive(Clone, Copy)]
struct Percent(f32);

impl std::str::FromStr for Percent {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let trimmed = raw.trim().trim_end_matches('%').trim();
        let value: f32 = trimmed
            .parse()
            .map_err(|_| format!("{raw:?} is not a number"))?;

        if !(0.0..=100.0).contains(&value) {
            return Err(format!("{value} is outside 0 to 100"));
        }
        Ok(Self(value))
    }
}

fn main() -> ExitCode {
    match run(&Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("klart: {problem}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    // Probing does not need a mechanism to have been resolved — the whole point
    // is the displays where none could be.
    if let Command::Autostart { wanted } = &cli.command {
        let state = match wanted {
            Some(Switch(wanted)) => klart_core::set_login_item(*wanted)?,
            None => klart_core::login_item(),
        };
        println!("{}", describe_autostart(state));
        return Ok(());
    }

    if let Command::Rename { display, name } = &cli.command {
        let found = controls()?;
        let chosen = select::resolve(&candidates(&found), &Selection::Named(display.clone()))?;

        let mut remembered = Remembered::load();
        for index in chosen {
            let key = found[index].display().key();
            remembered.rename(key, name.as_deref());
            println!(
                "{key} is now {}",
                name.as_deref()
                    .map_or_else(|| found[index].display().name(), |given| given)
            );
        }
        remembered.save()?;
        return Ok(());
    }

    if let Command::Probe { output } = &cli.command {
        let reports = klart_core::diagnose()?;
        if output.json {
            render::probe_json(&reports);
        } else {
            render::probe(&reports);
        }
        return Ok(());
    }

    let found = controls()?;

    // `list` on a machine with nothing attached is an answer, not a failure —
    // the same way an empty directory is not an error for `ls`. Every other
    // command genuinely has nothing to act on.
    if found.is_empty()
        && let Command::List { output } = &cli.command
    {
        if output.json {
            println!("[]");
        } else {
            println!("no displays are attached");
        }
        return Ok(());
    }

    let (target, output) = match &cli.command {
        Command::List { output } => (
            &Target {
                display: None,
                all: true,
            },
            output,
        ),
        Command::Get { target, output }
        | Command::Restore { target, output }
        | Command::Set { target, output, .. }
        | Command::Up { target, output, .. }
        | Command::Down { target, output, .. } => (target, output),

        // All three return above, before any of this.
        Command::Probe { .. } | Command::Autostart { .. } | Command::Rename { .. } => {
            unreachable!("these return before a target is needed")
        }
    };

    let chosen = select::resolve(&candidates(&found), &target.selection())?;
    let mut remembered = Remembered::load();

    for &index in &chosen {
        let control = &found[index];
        match &cli.command {
            Command::List { .. }
            | Command::Get { .. }
            | Command::Probe { .. }
            | Command::Autostart { .. }
            | Command::Rename { .. } => continue,
            Command::Set { percent, .. } => {
                control.set(Brightness::from_percent(percent.0))?;
            }
            Command::Up { step, .. } => {
                control.adjust(step / 100.0)?;
            }
            Command::Down { step, .. } => {
                control.adjust(-step / 100.0)?;
            }
            Command::Restore { .. } => {
                let Some(level) = remembered.level_for(control.display().key()) else {
                    continue;
                };
                control.set(level)?;
            }
        }

        // Recorded after the fact rather than from the argument, because `up`
        // and `down` saturate and `set` on a DDC monitor lands on the nearest
        // step of that monitor's own scale. What is stored should be where the
        // display actually is.
        if let Ok(landed) = control.get() {
            remembered.remember(control.display().key(), landed);
        }
    }

    if let Err(problem) = remembered.save() {
        // Worth saying and not worth failing for: the brightness did change.
        eprintln!("klart: could not save levels: {problem}");
    }

    let changed = !matches!(cli.command, Command::List { .. } | Command::Get { .. });
    let reported: Vec<&Control> = chosen.iter().map(|&index| &found[index]).collect();

    if output.json {
        render::json(&reported, &chosen);
    } else {
        render::table(
            &reported,
            &chosen,
            matches!(cli.command, Command::List { .. }),
        );
    }

    if changed {
        render::warn_about_anything_that_will_not_last(&reported);
    }
    Ok(())
}

fn candidates(found: &[Control]) -> Vec<Candidate> {
    found
        .iter()
        .map(|control| Candidate {
            name: control.name().to_owned(),
            key: control.display().key().to_string(),
            is_main: control.display().is_main(),
        })
        .collect()
}

/// What to say about the login item.
fn describe_autostart(state: LoginItem) -> &'static str {
    match state {
        LoginItem::Enabled => "on",
        LoginItem::Disabled => "off",
        LoginItem::AwaitingApproval => {
            "registered, waiting to be allowed in System Settings under General, Login Items"
        }
        LoginItem::Unavailable => {
            "unavailable — this is not running from the app bundle. Build it with \
             `scripts/bundle.sh` and run `Klart.app/Contents/MacOS/klart` instead."
        }
    }
}
