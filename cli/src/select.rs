//! Working out which displays a command was aimed at.
//!
//! Separate from the displays themselves so that it can be tested: every case
//! worth getting right here — an ambiguous name, an index that is one past the
//! end, a machine with nothing attached — is awkward to arrange with real
//! hardware and trivial to arrange with a list of names.

use std::fmt;

/// What the caller asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// The display holding the menu bar. The default, because a command with no
    /// target should act on the screen the person is looking at.
    Main,
    /// Every attached display.
    All,
    /// One display, by index, key or name.
    Named(String),
}

/// The part of a display this needs in order to choose between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub key: String,
    pub is_main: bool,
}

/// Why nothing could be chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The machine reports no displays at all.
    NothingAttached,
    /// No display answers to this.
    NoMatch(String),
    /// More than one does.
    Ambiguous {
        /// What the caller asked for.
        wanted: String,
        /// The displays that answered to it.
        matches: Vec<String>,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NothingAttached => write!(f, "no displays are attached"),
            Self::NoMatch(wanted) => {
                write!(f, "no display matches {wanted:?}; try `klart list`")
            }
            Self::Ambiguous { wanted, matches } => write!(
                f,
                "{:?} matches {} displays ({}); use an index or a key from `klart list`",
                wanted,
                matches.len(),
                matches.join(", ")
            ),
        }
    }
}

impl std::error::Error for Problem {}

/// The indices a selection picks out, in the order the displays were listed.
///
/// # Errors
///
/// Fails when nothing is attached, when nothing matches, and when a name is
/// ambiguous — the last of these rather than picking one, because guessing which
/// of two identical monitors was meant is worse than asking.
pub fn resolve(candidates: &[Candidate], selection: &Selection) -> Result<Vec<usize>, Problem> {
    if candidates.is_empty() {
        return Err(Problem::NothingAttached);
    }

    match selection {
        Selection::All => Ok((0..candidates.len()).collect()),

        Selection::Main => Ok(vec![
            candidates
                .iter()
                .position(|found| found.is_main)
                // A display list with nothing marked as main is not something
                // macOS produces, but falling back beats refusing to run.
                .unwrap_or(0),
        ]),

        Selection::Named(wanted) => resolve_one(candidates, wanted).map(|found| vec![found]),
    }
}

fn resolve_one(candidates: &[Candidate], wanted: &str) -> Result<usize, Problem> {
    // An index first, because it is what `klart list` prints and the only form
    // that can never be ambiguous.
    if let Ok(index) = wanted.parse::<usize>()
        && index < candidates.len()
    {
        return Ok(index);
    }

    // Then a key, which is exact by construction.
    if let Some(index) = candidates
        .iter()
        .position(|found| found.key.eq_ignore_ascii_case(wanted))
    {
        return Ok(index);
    }

    // Then a name. Exact before partial, so that a display whose full name is a
    // prefix of another's is still reachable by typing it out.
    let exact: Vec<usize> = matching(candidates, |name| name.eq_ignore_ascii_case(wanted));
    let matches = if exact.is_empty() {
        matching(candidates, |name| {
            name.to_lowercase().contains(&wanted.to_lowercase())
        })
    } else {
        exact
    };

    match matches.as_slice() {
        [] => Err(Problem::NoMatch(wanted.to_owned())),
        [only] => Ok(*only),
        several => Err(Problem::Ambiguous {
            wanted: wanted.to_owned(),
            matches: several
                .iter()
                .map(|&index| candidates[index].name.clone())
                .collect(),
        }),
    }
}

fn matching(candidates: &[Candidate], predicate: impl Fn(&str) -> bool) -> Vec<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, found)| predicate(&found.name))
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str, key: &str, is_main: bool) -> Candidate {
        Candidate {
            name: name.to_owned(),
            key: key.to_owned(),
            is_main,
        }
    }

    fn two() -> Vec<Candidate> {
        vec![
            candidate("LS32AG55x", "SAM-71e3-HNAW900001", true),
            candidate("Built-in Display", "builtin", false),
        ]
    }

    #[test]
    fn all_takes_everything_in_order() {
        assert_eq!(resolve(&two(), &Selection::All), Ok(vec![0, 1]));
    }

    #[test]
    fn the_default_is_the_display_holding_the_menu_bar() {
        assert_eq!(resolve(&two(), &Selection::Main), Ok(vec![0]));
    }

    #[test]
    fn a_list_with_no_main_display_still_resolves() {
        let headless = vec![candidate("One", "one", false)];
        assert_eq!(resolve(&headless, &Selection::Main), Ok(vec![0]));
    }

    #[test]
    fn nothing_attached_is_its_own_answer() {
        assert_eq!(resolve(&[], &Selection::All), Err(Problem::NothingAttached));
    }

    #[test]
    fn an_index_picks_the_display_that_list_printed() {
        assert_eq!(
            resolve(&two(), &Selection::Named("1".to_owned())),
            Ok(vec![1])
        );
    }

    #[test]
    fn an_index_past_the_end_falls_through_to_the_name_search() {
        // And finds nothing, rather than panicking on the subscript.
        assert_eq!(
            resolve(&two(), &Selection::Named("7".to_owned())),
            Err(Problem::NoMatch("7".to_owned()))
        );
    }

    #[test]
    fn a_key_matches_exactly_and_without_regard_to_case() {
        assert_eq!(
            resolve(&two(), &Selection::Named("sam-71e3-hnaw900001".to_owned())),
            Ok(vec![0])
        );
    }

    #[test]
    fn part_of_a_name_is_enough_when_it_is_unambiguous() {
        assert_eq!(
            resolve(&two(), &Selection::Named("built".to_owned())),
            Ok(vec![1])
        );
    }

    #[test]
    fn an_ambiguous_name_is_refused_rather_than_guessed() {
        let twins = vec![
            candidate("Dell U2723QE", "DEL-1-a", true),
            candidate("Dell U2723QE", "DEL-1-b", false),
        ];

        let refused = resolve(&twins, &Selection::Named("dell".to_owned()));

        assert_eq!(
            refused,
            Err(Problem::Ambiguous {
                wanted: "dell".to_owned(),
                matches: vec!["Dell U2723QE".to_owned(), "Dell U2723QE".to_owned()],
            })
        );
    }

    #[test]
    fn an_exact_name_beats_a_partial_one() {
        // Without this, a display whose whole name is a prefix of another's
        // could not be selected by name at all.
        let overlapping = vec![
            candidate("Studio Display", "a", true),
            candidate("Studio Display Pro", "b", false),
        ];

        assert_eq!(
            resolve(&overlapping, &Selection::Named("Studio Display".to_owned())),
            Ok(vec![0])
        );
    }

    #[test]
    fn a_key_is_preferred_over_a_name_that_happens_to_contain_it() {
        let confusing = vec![
            candidate("builtin lookalike", "SAM-1-x", true),
            candidate("Built-in Display", "builtin", false),
        ];

        assert_eq!(
            resolve(&confusing, &Selection::Named("builtin".to_owned())),
            Ok(vec![1])
        );
    }
}
