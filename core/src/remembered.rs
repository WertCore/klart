//! Levels that outlive the process.
//!
//! Small enough to be its own format rather than a library's. The whole file is
//! a display key, an equals sign and a value, one line at a time, and the keys
//! are constrained by [`DisplayKey`]'s contract to characters that need no
//! quoting — so a parser is fifty lines and a dependency would be a larger
//! surface than the thing it parsed.
//!
//! A key with `:name` after it is the display's name rather than its level. The
//! colon is deliberate: it is not in the character set a key can contain, so it
//! cannot occur inside one and no escaping is needed to tell the two apart.
//!
//! It is also meant to be edited by hand, and read on another operating system:
//! the keys come out of EDID, so a file written on macOS describes the same
//! displays on Windows.
//!
//! Nothing here fails loudly. A missing file is an empty one, and a line that
//! makes no sense is skipped and complained about rather than thrown — the worst
//! case is a display that comes back at the level its own buttons left it, which
//! is exactly where it would be without any of this.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use crate::identity::DisplayKey;
use crate::{Brightness, platform};

/// The file's name inside the configuration directory.
const FILE: &str = "levels.conf";

/// What was written at the top of a file this wrote.
const PREAMBLE: &str = "\
# What klart remembers about each display.
#
# `key = percent` is the level to put it back to, and `key:name = ...` is what to
# call it. The keys are the ones `klart list` prints; they come from the display's
# EDID, so they mean the same thing on any operating system.
";

/// What marks a line as a name rather than a level.
///
/// A colon cannot appear in a display key — [`DisplayKey`]'s contract replaces
/// anything outside `A-Z a-z 0-9 . _ -` — so this can never be ambiguous with
/// part of a key.
const NAME_SUFFIX: &str = ":name";

/// The level each display was last left at.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Remembered {
    levels: BTreeMap<String, u8>,
    names: BTreeMap<String, String>,
    /// Whether anything has changed since this was loaded or saved.
    dirty: bool,
}

impl Remembered {
    /// Reads the file, or starts empty if there is not one.
    ///
    /// Never fails. Anything unreadable is reported on stderr and treated as
    /// absent, because refusing to start over a configuration file would be a
    /// worse outcome than forgetting what was in it.
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = path() else {
            return Self::default();
        };

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
                return Self::default();
            }
            Err(problem) => {
                eprintln!("klart: could not read {}: {problem}", path.display());
                return Self::default();
            }
        };

        let (levels, names, complaints) = parse(&text);
        for complaint in complaints {
            eprintln!("klart: {}: {complaint}", path.display());
        }

        Self {
            levels,
            names,
            dirty: false,
        }
    }

    /// What this display should be called, if it has been renamed.
    #[must_use]
    pub fn name_for(&self, key: &DisplayKey) -> Option<&str> {
        self.names.get(key.as_str()).map(String::as_str)
    }

    /// Renames a display, or with [`None`] gives it its own name back.
    ///
    /// A name that is only whitespace counts as clearing it, because a display
    /// with a blank name cannot be picked out of a list.
    pub fn rename(&mut self, key: &DisplayKey, name: Option<&str>) {
        let name = name.map(str::trim).filter(|name| !name.is_empty());

        let changed = match name {
            Some(name) => {
                self.names.insert(key.as_str().to_owned(), name.to_owned()) != Some(name.to_owned())
            }
            None => self.names.remove(key.as_str()).is_some(),
        };
        self.dirty |= changed;
    }

    /// The level this display was last left at.
    #[must_use]
    pub fn level_for(&self, key: &DisplayKey) -> Option<Brightness> {
        self.levels
            .get(key.as_str())
            .map(|&percent| Brightness::from_percent(f32::from(percent)))
    }

    /// Records where a display has been put.
    pub fn remember(&mut self, key: &DisplayKey, level: Brightness) {
        let percent = level.percent_rounded();
        if self.levels.insert(key.as_str().to_owned(), percent) != Some(percent) {
            self.dirty = true;
        }
    }

    /// Whether anything has changed since this was loaded or saved.
    ///
    /// The menu bar agent writes a level on every step of a slider drag, and
    /// there is no reason for the file system to hear about all of them.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Writes the file, if anything has changed.
    ///
    /// Written to a temporary file and renamed over the old one, so that a crash
    /// or a full disk leaves the previous file rather than half of a new one.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be created or the file cannot be written.
    pub fn save(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }

        let Some(path) = path() else {
            return Ok(());
        };
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }

        let temporary = path.with_extension("conf.new");
        std::fs::write(&temporary, render(&self.levels, &self.names))?;
        std::fs::rename(&temporary, &path)?;

        self.dirty = false;
        Ok(())
    }
}

/// Where the file lives.
///
/// [`None`] when the platform will not say where configuration belongs, which
/// means remembering is off rather than that anything has gone wrong.
#[must_use]
pub fn path() -> Option<PathBuf> {
    Some(platform::config_directory()?.join(FILE))
}

/// Reads the file's contents, and says what it could not read.
type Parsed = (BTreeMap<String, u8>, BTreeMap<String, String>, Vec<String>);

fn parse(text: &str) -> Parsed {
    let mut levels = BTreeMap::new();
    let mut names = BTreeMap::new();
    let mut complaints = Vec::new();

    for (number, line) in text.lines().enumerate() {
        let line = line.trim();

        // Only a leading `#` is a comment. Display keys can contain one — that
        // is how two identical monitors are told apart — and treating it as a
        // comment anywhere would silently drop exactly those displays.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            complaints.push(format!("line {}: no `=`, ignoring", number + 1));
            continue;
        };

        let key = key.trim();
        if key.is_empty() {
            complaints.push(format!("line {}: no display key, ignoring", number + 1));
            continue;
        }

        // A name, which is free text and only has to be trimmed.
        if let Some(named) = key.strip_suffix(NAME_SUFFIX) {
            let named = named.trim();
            let value = value.trim();
            if named.is_empty() || value.is_empty() {
                complaints.push(format!("line {}: empty name, ignoring", number + 1));
            } else {
                names.insert(named.to_owned(), value.to_owned());
            }
            continue;
        }

        match value.trim().parse::<u8>() {
            Ok(percent) if percent <= 100 => {
                // Last wins. A hand-edited file with a repeated key most likely
                // has the newer intention at the bottom.
                levels.insert(key.to_owned(), percent);
            }
            _ => complaints.push(format!(
                "line {}: {:?} is not a percentage, ignoring",
                number + 1,
                value.trim()
            )),
        }
    }

    (levels, names, complaints)
}

/// Writes the file's contents.
fn render(levels: &BTreeMap<String, u8>, names: &BTreeMap<String, String>) -> String {
    let mut text = String::from(PREAMBLE);

    // `BTreeMap`, so the order is the keys' and a file written twice from the
    // same state is byte-identical.
    for (key, percent) in levels {
        let _ = writeln!(text, "{key} = {percent}");
        if let Some(name) = names.get(key) {
            let _ = writeln!(text, "{key}{NAME_SUFFIX} = {name}");
        }
    }

    // A display that has been renamed but never had a level set still needs its
    // name written out.
    for (key, name) in names {
        if !levels.contains_key(key) {
            let _ = writeln!(text, "{key}{NAME_SUFFIX} = {name}");
        }
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_this_wrote_is_one_it_can_read() {
        let mut levels = BTreeMap::new();
        levels.insert("builtin".to_owned(), 44);
        levels.insert("SAM-71e3-HNAW900001".to_owned(), 70);

        let (read_back, _, complaints) = parse(&render(&levels, &BTreeMap::new()));

        assert_eq!(read_back, levels);
        assert!(complaints.is_empty(), "{complaints:?}");
    }

    #[test]
    fn a_key_containing_a_disambiguator_survives_the_round_trip() {
        // The case that makes `#` a leading-only comment marker: two identical
        // monitors are told apart by a `#` inside the key.
        let mut levels = BTreeMap::new();
        levels.insert("SAM-71e3-00000000#1".to_owned(), 30);
        levels.insert("SAM-71e3-00000000#2".to_owned(), 60);

        let (read_back, _, complaints) = parse(&render(&levels, &BTreeMap::new()));

        assert_eq!(read_back, levels);
        assert!(complaints.is_empty(), "{complaints:?}");
    }

    #[test]
    fn comments_and_blank_lines_are_skipped_without_complaint() {
        let (levels, _, complaints) = parse("# a note\n\n   \nbuiltin = 10\n");

        assert_eq!(levels.get("builtin"), Some(&10));
        assert!(complaints.is_empty(), "{complaints:?}");
    }

    #[test]
    fn whitespace_around_either_side_is_not_part_of_anything() {
        let (levels, _, _) = parse("   builtin   =   10   \n");
        assert_eq!(levels.get("builtin"), Some(&10));
    }

    #[test]
    fn a_file_with_windows_line_endings_reads_the_same() {
        // The whole point of the key contract is that this file crosses between
        // operating systems, so it will meet CRLF sooner or later.
        let (levels, _, complaints) = parse("builtin = 10\r\nSAM-1-x = 20\r\n");

        assert_eq!(levels.get("builtin"), Some(&10));
        assert_eq!(levels.get("SAM-1-x"), Some(&20));
        assert!(complaints.is_empty(), "{complaints:?}");
    }

    #[test]
    fn a_line_that_makes_no_sense_is_skipped_and_named() {
        let (levels, _, complaints) = parse(
            "builtin = 10\n\
             nonsense\n\
             = 40\n\
             other = wat\n\
             toobig = 400\n\
             kept = 20\n",
        );

        // The lines either side of the bad ones still land.
        assert_eq!(levels.get("builtin"), Some(&10));
        assert_eq!(levels.get("kept"), Some(&20));
        assert_eq!(levels.len(), 2);

        assert_eq!(complaints.len(), 4, "{complaints:?}");
        assert!(complaints[0].starts_with("line 2:"), "{complaints:?}");
        assert!(complaints[1].starts_with("line 3:"), "{complaints:?}");
        assert!(complaints[2].starts_with("line 4:"), "{complaints:?}");
        assert!(complaints[3].starts_with("line 5:"), "{complaints:?}");
    }

    #[test]
    fn a_repeated_key_takes_the_last_value() {
        let (levels, _, _) = parse("builtin = 10\nbuiltin = 90\n");
        assert_eq!(levels.get("builtin"), Some(&90));
    }

    #[test]
    fn remembering_the_same_level_twice_does_not_dirty_anything() {
        let mut remembered = Remembered::default();
        let key = DisplayKey::of(&crate::identity::Identity {
            built_in: true,
            manufacturer: 0,
            product: 0,
            serial: 0,
            printed_serial: None,
        });

        remembered.remember(&key, Brightness::from_percent(40.0));
        assert!(remembered.is_dirty());

        remembered.dirty = false;
        remembered.remember(&key, Brightness::from_percent(40.0));
        assert!(
            !remembered.is_dirty(),
            "a drag writes the same value many times over"
        );

        remembered.remember(&key, Brightness::from_percent(41.0));
        assert!(remembered.is_dirty());
    }

    #[test]
    fn a_remembered_level_comes_back_as_the_level_that_was_put_in() {
        let mut remembered = Remembered::default();
        let key = DisplayKey::of(&crate::identity::Identity {
            built_in: false,
            manufacturer: 0x4c2d,
            product: 0x71e3,
            serial: 1,
            printed_serial: None,
        });

        remembered.remember(&key, Brightness::from_percent(65.0));

        assert_eq!(
            remembered
                .level_for(&key)
                .map(|level| level.percent_rounded()),
            Some(65)
        );
    }

    #[test]
    fn a_name_survives_the_round_trip_alongside_a_level() {
        let mut levels = BTreeMap::new();
        levels.insert("builtin".to_owned(), 44);
        let mut names = BTreeMap::new();
        names.insert("builtin".to_owned(), "Laptop".to_owned());

        let (read_levels, read_names, complaints) = parse(&render(&levels, &names));

        assert_eq!(read_levels, levels);
        assert_eq!(read_names, names);
        assert!(complaints.is_empty(), "{complaints:?}");
    }

    #[test]
    fn a_name_is_written_even_for_a_display_with_no_level() {
        let mut names = BTreeMap::new();
        names.insert("SAM-1-x".to_owned(), "Desk".to_owned());

        let (_, read_names, _) = parse(&render(&BTreeMap::new(), &names));

        assert_eq!(read_names, names);
    }

    #[test]
    fn a_name_may_contain_the_characters_a_key_may_not() {
        // The value is free text. Only the key side is constrained, which is
        // what makes the colon safe as the marker.
        let mut names = BTreeMap::new();
        names.insert(
            "builtin".to_owned(),
            "Desk = the one on the left".to_owned(),
        );

        let (_, read_names, complaints) = parse(&render(&BTreeMap::new(), &names));

        assert_eq!(
            read_names.get("builtin").map(String::as_str),
            Some("Desk = the one on the left"),
            "splitting on the first `=` is what makes this work"
        );
        assert!(complaints.is_empty(), "{complaints:?}");
    }

    #[test]
    fn renaming_to_nothing_gives_a_display_its_own_name_back() {
        let mut remembered = Remembered::default();
        let key = DisplayKey::of(&crate::identity::Identity {
            built_in: true,
            manufacturer: 0,
            product: 0,
            serial: 0,
            printed_serial: None,
        });

        remembered.rename(&key, Some("Laptop"));
        assert_eq!(remembered.name_for(&key), Some("Laptop"));

        remembered.rename(&key, Some("   "));
        assert_eq!(
            remembered.name_for(&key),
            None,
            "a blank name cannot be picked out of a list"
        );
    }
}
