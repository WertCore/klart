//! Software dimming, through the display's gamma ramp.
//!
//! `SetDeviceGammaRamp` on a device context for the adapter. The same trade as
//! everywhere else: it darkens the picture rather than the backlight, so
//! contrast goes with it, and it is what is left when nothing better answers.
//!
//! Unlike macOS, Windows does not put the ramp back when the process that set it
//! exits — it stays until something else changes it or the session ends. That is
//! useful for a command line, and dangerous for a resident agent: a crash must
//! not leave a display dark with nobody left to undo it. So the ramp this writes
//! is a scaled copy of whatever was there before, the original is kept, and the
//! agent releases it on the way out.
//!
//! The original cannot be read back from the display once it has been scaled
//! without knowing what it was. It is therefore written to `gamma.conf` next to
//! the remembered levels, keyed by display, before the first dimming write. A
//! process that finds that file after a crash can tell its own scaled ramp from
//! a fresh calibration change and put the original back.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use windows_sys::Win32::Graphics::Gdi::{CreateDCW, DeleteDC, HDC};
use windows_sys::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};

use crate::Brightness;
use crate::backend::Backend;
use crate::error::{Error, Result};

use super::monitors::to_wide;

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "gamma";

/// The dimmest ramp this will write.
///
/// The same floor, and for the same reason, as every other platform: a ramp of
/// zero is a black screen, and a black screen cannot be undone from a menu
/// nobody can see.
const FLOOR: f32 = 0.2;

/// How many entries a Windows gamma ramp has, per channel.
const ENTRIES: usize = 256;

/// The whole table, three interleaved channels.
const TABLE: usize = ENTRIES * 3;

/// How much a read-back may differ from what was written and still count.
///
/// A gamma ramp is occasionally quantised to eight bits by a driver, which is a
/// difference of 257 in a `u16`. This is a little wider than that: enough to
/// absorb the quantisation, not enough to mistake a different curve for the one
/// that was written.
const TOLERANCE: u16 = 1024;

/// How far the factor read back may sit from the one asked for.
const FACTOR_TOLERANCE: f32 = 0.02;

/// The file holding ramp baselines that a dimmed display needs to be undone.
const STATE_FILE: &str = "gamma.conf";

/// One display's original ramp, before klart scaled it.
type Ramp = [u16; TABLE];

/// Serialises reads, writes and state-file updates across every gamma instance.
///
/// The tray opens two sets of controls — one for the message loop and one for
/// the writer thread — so two `Gamma`s can address the same adapter. The display
/// call itself is short; holding one lock around it is simpler than teaching
/// every caller which of its controls are the same display.
static GAMMA_LOCK: Mutex<()> = Mutex::new(());

/// A display's brightness, faked with its gamma ramp.
pub(crate) struct Gamma {
    adapter: String,
    key: String,
    /// The ramp that was there before klart first dimmed this display.
    baseline: RefCell<Ramp>,
    /// The last table this instance wrote, for telling its own ramp from a
    /// change made by something else.
    last_written: RefCell<Option<Ramp>>,
    /// The level the last write was for, so a read-back need not be inverted
    /// through the factor if nothing has changed since.
    last_level: Cell<f32>,
}

impl Gamma {
    /// Binds to an adapter for one display.
    ///
    /// # Errors
    ///
    /// When a device context cannot be made for it, or its ramp cannot be read,
    /// which means the adapter has gone away.
    pub(crate) fn open(adapter: &str, key: &str) -> Result<Self> {
        let opened = Self {
            adapter: adapter.to_owned(),
            key: key.to_owned(),
            baseline: RefCell::new([0; TABLE]),
            last_written: RefCell::new(None),
            last_level: Cell::new(1.0),
        };

        let current = opened.with_context(read)?;
        let _guard = lock();

        // A saved baseline is only ours if the ramp in front of us is that
        // baseline or a scaled copy of it. If it is neither, something else
        // changed the calibration while nothing was holding the ramp; that is
        // the new baseline, and overwriting it on the next quit would clobber
        // exactly the change this exists to preserve.
        let saved = load_state().remove(&opened.key);
        let (baseline, level, last_written) = match saved {
            Some(saved) if resembles(&current, &saved) => {
                let factor = factor_of(&current, &saved);
                (saved, level_from_factor(factor), Some(current))
            }
            Some(_) => {
                let _ = remove_saved(&opened.key);
                (current, 1.0, None)
            }
            None => (current, 1.0, None),
        };

        *opened.baseline.borrow_mut() = baseline;
        *opened.last_written.borrow_mut() = last_written;
        opened.last_level.set(level);
        Ok(opened)
    }

    /// Runs something against a device context for this adapter.
    ///
    /// The context is created and destroyed around each use rather than held.
    /// A device context is a scarce, session-scoped resource and this is an
    /// agent that lives for the whole login; keeping one open for hours to save
    /// a microsecond would be the wrong trade.
    fn with_context<T>(&self, body: impl FnOnce(HDC) -> Result<T>) -> Result<T> {
        let name = to_wide(&self.adapter);

        // SAFETY: `name` is NUL terminated and live across the call. The three
        // nulls are the documented way to ask for a context on the whole
        // adapter rather than a particular device or mode.
        let context = unsafe {
            CreateDCW(
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };

        if context.is_null() {
            return Err(Error::CannotReach {
                mechanism: NAME,
                display: self.adapter.clone(),
            });
        }

        let outcome = body(context);

        // SAFETY: the context came from `CreateDCW` and is deleted exactly once.
        unsafe { DeleteDC(context) };
        outcome
    }

    /// Chooses the baseline that the ramp in front of us belongs to.
    ///
    /// Kept apart from the display work so the three cases — our last write,
    /// our saved baseline, and somebody else's change — are visible together.
    fn baseline_for(&self, current: &Ramp, saved: Option<&Ramp>) -> Ramp {
        if let Some(last) = self.last_written.borrow().as_ref()
            && same(current, last)
        {
            return *self.baseline.borrow();
        }

        if let Some(saved) = saved
            && resembles(current, saved)
        {
            return *saved;
        }

        if resembles(current, &self.baseline.borrow()) {
            return *self.baseline.borrow();
        }

        // Nothing we know about explains this table. Something else changed the
        // calibration, so this is the state to scale from and later restore.
        *current
    }
}

impl Backend for Gamma {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self) -> Result<Brightness> {
        let current = self.with_context(read)?;
        let baseline = *self.baseline.borrow();

        // A table that is exactly what this instance wrote is the best answer;
        // otherwise invert the scale. A driver that quantises a little makes the
        // two differ by a few counts, which the factor path absorbs.
        if let Some(last) = self.last_written.borrow().as_ref()
            && same(&current, last)
        {
            return Ok(Brightness::new(self.last_level.get()));
        }

        Ok(Brightness::new(level_from_factor(factor_of(
            &current, &baseline,
        ))))
    }

    fn set(&self, level: Brightness) -> Result<()> {
        let _guard = lock();
        let current = self.with_context(read)?;
        let saved = load_state().remove(&self.key);
        let baseline = self.baseline_for(&current, saved.as_ref());

        if !same(&baseline, &self.baseline.borrow()) {
            *self.baseline.borrow_mut() = baseline;
        }

        let factor = to_factor(level);
        let wanted = scale(&baseline, factor);

        // Save before the first dimming write, not after. A crash between the
        // two is exactly the case this file exists for, and a baseline written
        // for a level that never reached the display is harmless: it is checked
        // against the ramp on the next open and discarded if it does not match.
        // A drag writes the same baseline many times, so only touch the file
        // when the saved curve is not already this one.
        if factor < 1.0 && saved.as_ref() != Some(&baseline) {
            save_baseline(&self.key, &baseline)?;
        }

        let observed = self.with_context(|context| {
            write(context, &wanted)?;
            read(context)
        })?;

        if !agrees(&observed, &wanted, &baseline, factor) {
            // Put back what was there before. This is the one place where a
            // failed write could otherwise leave a display darker than either
            // its old state or the one asked for.
            let _ = self.with_context(|context| write(context, &current));
            return Err(Error::MechanismFailed {
                mechanism: NAME,
                call: "SetDeviceGammaRamp (read-back)",
                code: -1,
            });
        }

        *self.last_written.borrow_mut() = Some(wanted);
        self.last_level.set(level.fraction());

        if factor >= 1.0 {
            remove_saved(&self.key)?;
        }

        Ok(())
    }

    fn release(&self) -> Result<()> {
        let _guard = lock();
        let current = self.with_context(read)?;
        let baseline = *self.baseline.borrow();
        let ours = self
            .last_written
            .borrow()
            .as_ref()
            .is_some_and(|last| same(&current, last))
            || resembles(&current, &baseline);

        if ours {
            self.with_context(|context| {
                write(context, &baseline)?;
                let observed = read(context)?;
                if same(&observed, &baseline) {
                    Ok(())
                } else {
                    Err(Error::MechanismFailed {
                        mechanism: NAME,
                        call: "SetDeviceGammaRamp (release)",
                        code: -1,
                    })
                }
            })?;
        }

        *self.last_written.borrow_mut() = None;
        *self.baseline.borrow_mut() = if ours { baseline } else { current };
        self.last_level.set(1.0);
        remove_saved(&self.key)
    }
}

/// Reads the ramp for a device context.
fn read(context: HDC) -> Result<Ramp> {
    let mut ramp = [0_u16; TABLE];

    // SAFETY: the buffer is the 3 × 256 × `u16` this call documents.
    let ok = unsafe { GetDeviceGammaRamp(context, ramp.as_mut_ptr().cast()) };
    if ok == 0 {
        return Err(Error::MechanismFailed {
            mechanism: NAME,
            call: "GetDeviceGammaRamp",
            code: -1,
        });
    }

    Ok(ramp)
}

/// Writes a ramp for a device context.
fn write(context: HDC, ramp: &Ramp) -> Result<()> {
    // SAFETY: the buffer is the 3 × 256 × `u16` this call documents.
    let ok = unsafe { SetDeviceGammaRamp(context, ramp.as_ptr().cast()) };
    if ok == 0 {
        return Err(Error::MechanismFailed {
            mechanism: NAME,
            call: "SetDeviceGammaRamp",
            code: -1,
        });
    }
    Ok(())
}

/// The ramp factor that shows a given level.
fn to_factor(level: Brightness) -> f32 {
    FLOOR + (1.0 - FLOOR) * level.fraction()
}

/// The level a given factor is showing.
fn level_from_factor(factor: f32) -> f32 {
    ((factor - FLOOR) / (1.0 - FLOOR)).clamp(0.0, 1.0)
}

/// Scales a baseline table towards black.
fn scale(baseline: &Ramp, factor: f32) -> Ramp {
    let mut scaled = [0_u16; TABLE];
    for (entry, base) in scaled.iter_mut().zip(baseline) {
        *entry = (f32::from(*base) * factor)
            .round()
            .clamp(0.0, f32::from(u16::MAX)) as u16;
    }
    scaled
}

/// The scale factor a table appears to have relative to a baseline.
fn factor_of(table: &Ramp, baseline: &Ramp) -> f32 {
    let ceiling = baseline.iter().copied().max().unwrap_or(0);
    if ceiling == 0 {
        return 1.0;
    }
    f32::from(table.iter().copied().max().unwrap_or(0)) / f32::from(ceiling)
}

/// Whether two tables are the same curve within the tolerance of the API.
fn same(left: &Ramp, right: &Ramp) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| left.abs_diff(*right) <= TOLERANCE)
}

/// Whether a table is a baseline or a scaled copy of it.
fn resembles(table: &Ramp, baseline: &Ramp) -> bool {
    let factor = factor_of(table, baseline);
    same(table, baseline) || same(table, &scale(baseline, factor))
}

/// Whether a read-back is the curve that was written.
fn agrees(observed: &Ramp, wanted: &Ramp, baseline: &Ramp, factor: f32) -> bool {
    same(observed, wanted)
        || (factor_of(observed, baseline) - factor).abs() <= FACTOR_TOLERANCE
            && resembles(observed, baseline)
}

/// Takes the process-wide gamma lock, recovering a poisoned one.
fn lock() -> MutexGuard<'static, ()> {
    GAMMA_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The path of the ramp-baseline file, when this platform has a configuration
/// directory.
fn state_path() -> Option<PathBuf> {
    Some(super::config_directory()?.join(STATE_FILE))
}

/// Reads every saved baseline. A missing or unreadable file is an empty one:
/// the worst case is a ramp that cannot be recovered automatically after a
/// crash, which is why the floor exists.
fn load_state() -> BTreeMap<String, Ramp> {
    let Some(path) = state_path() else {
        return BTreeMap::new();
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
            return BTreeMap::new();
        }
        Err(problem) => {
            eprintln!("klart: could not read {}: {problem}", path.display());
            return BTreeMap::new();
        }
    };

    parse_state(&text)
}

/// Parses the state file, ignoring lines it cannot understand.
fn parse_state(text: &str) -> BTreeMap<String, Ramp> {
    let mut baselines = BTreeMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        if let Some(ramp) = decode(value.trim()) {
            baselines.insert(key.to_owned(), ramp);
        }
    }

    baselines
}

/// Writes the state file, leaving the old one in place if anything fails.
fn write_state(baselines: &BTreeMap<String, Ramp>) -> Result<()> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|_| Error::MechanismFailed {
            mechanism: NAME,
            call: "create_dir_all",
            code: -1,
        })?;
    }

    let temporary = path.with_extension("conf.new");
    std::fs::write(&temporary, render_state(baselines)).map_err(|_| Error::MechanismFailed {
        mechanism: NAME,
        call: "write gamma.conf",
        code: -1,
    })?;
    std::fs::rename(&temporary, &path).map_err(|_| Error::MechanismFailed {
        mechanism: NAME,
        call: "rename gamma.conf",
        code: -1,
    })?;
    Ok(())
}

/// Adds or replaces one saved baseline.
fn save_baseline(key: &str, ramp: &Ramp) -> Result<()> {
    let mut baselines = load_state();
    baselines.insert(key.to_owned(), *ramp);
    write_state(&baselines)
}

/// Forgets one saved baseline.
fn remove_saved(key: &str) -> Result<()> {
    let mut baselines = load_state();
    if baselines.remove(key).is_some() {
        write_state(&baselines)?;
    }
    Ok(())
}

/// Renders the state file.
fn render_state(baselines: &BTreeMap<String, Ramp>) -> String {
    let mut text = String::from(
        "# Original gamma ramps that klart scaled down.\n\
         # Written before dimming and removed when the ramp is given back, so a\n\
         # crash can be recovered from. Editing this by hand is not useful.\n",
    );

    for (key, ramp) in baselines {
        let _ = writeln!(text, "{key} = {}", encode(ramp));
    }
    text
}

/// A ramp as one line of four-hex-digit entries.
fn encode(ramp: &Ramp) -> String {
    let mut text = String::with_capacity(TABLE * 4);
    for entry in ramp {
        let _ = write!(text, "{entry:04x}");
    }
    text
}

/// The inverse of [`encode`], rejecting anything that is not exactly a ramp.
fn decode(text: &str) -> Option<Ramp> {
    if text.len() != TABLE * 4 || !text.is_ascii() {
        return None;
    }

    let mut ramp = [0_u16; TABLE];
    for (index, entry) in ramp.iter_mut().enumerate() {
        let start = index * 4;
        *entry = u16::from_str_radix(&text[start..start + 4], 16).ok()?;
    }
    Some(ramp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Ramp {
        let mut ramp = [0_u16; TABLE];
        for entry in 0..ENTRIES {
            let value = (entry as f32 / (ENTRIES - 1) as f32 * f32::from(u16::MAX)).round() as u16;
            ramp[entry] = value;
            ramp[ENTRIES + entry] = value;
            ramp[ENTRIES * 2 + entry] = value;
        }
        ramp
    }

    #[test]
    fn levels_round_trip_through_the_factor() {
        for whole in 0..=100u8 {
            let level = Brightness::from_percent(f32::from(whole));
            assert_eq!(
                Brightness::new(level_from_factor(to_factor(level))).percent_rounded(),
                whole
            );
        }
    }

    #[test]
    fn the_dimmest_level_still_leaves_something_on_the_screen() {
        assert_eq!(to_factor(Brightness::MIN), FLOOR);
        assert!(to_factor(Brightness::MIN) > 0.0);
    }

    #[test]
    fn the_brightest_level_is_the_ramp_untouched() {
        assert_eq!(to_factor(Brightness::MAX), 1.0);
        let baseline = identity();
        assert_eq!(scale(&baseline, 1.0), baseline);
    }

    #[test]
    fn a_scaled_ramp_is_recognised_as_a_scaled_ramp() {
        let baseline = identity();
        let scaled = scale(&baseline, 0.4);

        assert!(resembles(&scaled, &baseline));
        assert!((factor_of(&scaled, &baseline) - 0.4).abs() < 0.001);
    }

    #[test]
    fn a_different_curve_is_not_mistaken_for_our_dimmed_one() {
        let baseline = identity();
        let mut changed = baseline;
        // A calibration change that is not a uniform scale: crush the blue
        // channel, which is what a colour profile does.
        for entry in &mut changed[ENTRIES * 2..] {
            *entry /= 2;
        }

        assert!(
            !resembles(&changed, &baseline),
            "a changed curve must become the new baseline rather than be restored over"
        );
    }

    #[test]
    fn a_ramp_survives_the_state_file_round_trip() {
        let baseline = identity();
        let text = render_state(&BTreeMap::from([(
            "SAM-71e3-HNAW900001".to_owned(),
            baseline,
        )]));
        let read_back = parse_state(&text);

        assert_eq!(read_back.get("SAM-71e3-HNAW900001"), Some(&baseline));
    }

    #[test]
    fn a_truncated_ramp_is_ignored_rather_than_half_read() {
        let text = "SAM = 0000\n";
        assert!(parse_state(text).is_empty());
    }

    #[test]
    fn state_keys_survive_a_hash() {
        // Two identical monitors are told apart by a `#` in the key, which is
        // why this is not treated as a comment when it is not leading.
        let baseline = identity();
        let text = render_state(&BTreeMap::from([(
            "SAM-71e3-00000000#1".to_owned(),
            baseline,
        )]));
        assert_eq!(
            parse_state(&text).get("SAM-71e3-00000000#1"),
            Some(&baseline)
        );
    }
}
