use std::fmt;

/// A brightness level, as a fraction of a display's own range.
///
/// Backends disagree about units. `DisplayServices` takes a float, DDC/CI takes
/// an integer measured against a maximum the monitor reports for itself, and a
/// gamma ramp is a curve rather than a level at all. A fraction is the only
/// currency all three can be converted into without losing the others, so it is
/// the one the rest of the crate speaks.
///
/// The value is always in `0.0..=1.0`: every constructor clamps, so there is no
/// way to build one that a backend then has to re-validate.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Brightness(f32);

impl Brightness {
    /// The dimmest level a display will accept.
    ///
    /// Note that this is not "off" — a backlight at zero is still lit on most
    /// panels, and on the built-in display macOS itself refuses to go dark.
    pub const MIN: Self = Self(0.0);

    /// The brightest level a display will accept.
    pub const MAX: Self = Self(1.0);

    /// Builds a level from a fraction, clamping anything outside `0.0..=1.0`.
    ///
    /// A NaN becomes [`Brightness::MIN`]. Backends read these values off an I2C
    /// wire and out of a private framework, and neither is a source that can be
    /// trusted to return a number; turning a bad read into the dimmest setting
    /// is recoverable, whereas letting NaN travel is a panic several frames from
    /// where it came in.
    #[must_use]
    pub fn new(fraction: f32) -> Self {
        if fraction.is_nan() {
            return Self::MIN;
        }
        Self(fraction.clamp(0.0, 1.0))
    }

    /// Builds a level from a percentage, clamping anything outside `0.0..=100.0`.
    #[must_use]
    pub fn from_percent(percent: f32) -> Self {
        Self::new(percent / 100.0)
    }

    /// The level as a fraction in `0.0..=1.0`.
    #[must_use]
    pub fn fraction(self) -> f32 {
        self.0
    }

    /// The level as a percentage in `0.0..=100.0`.
    #[must_use]
    pub fn percent(self) -> f32 {
        self.0 * 100.0
    }

    /// The level as a whole percentage, for display to a person.
    #[must_use]
    pub fn percent_rounded(self) -> u8 {
        // The fraction is clamped, so the product is in `0.0..=100.0` and the
        // cast cannot saturate.
        self.percent().round() as u8
    }

    /// Scales the level onto `0..=max`, the form DDC/CI carries it in.
    ///
    /// Monitors report their own maximum for each control and they do not agree
    /// on it — 100 is common, so is 65535 — so the maximum is a parameter rather
    /// than a constant.
    #[must_use]
    pub fn to_range(self, max: u16) -> u16 {
        (self.0 * f32::from(max)).round() as u16
    }

    /// Reads a level back from `0..=max`.
    ///
    /// A `max` of zero is what a monitor reports when it does not really support
    /// the control, so it yields [`Brightness::MIN`] rather than dividing by it.
    /// `value` above `max` is out of spec but does happen, and is treated as the
    /// maximum rather than rejected.
    #[must_use]
    pub fn from_range(value: u16, max: u16) -> Self {
        if max == 0 {
            return Self::MIN;
        }
        Self::new(f32::from(value.min(max)) / f32::from(max))
    }

    /// Moves the level by `delta`, saturating at both ends.
    ///
    /// Saturating rather than wrapping is what makes a held-down key behave:
    /// pressing "brighter" at full brightness should do nothing, not jump to
    /// black.
    #[must_use]
    pub fn stepped(self, delta: f32) -> Self {
        Self::new(self.0 + delta)
    }
}

impl fmt::Display for Brightness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}%", self.percent_rounded())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_outside_the_unit_interval_are_clamped() {
        assert_eq!(Brightness::new(-0.5), Brightness::MIN);
        assert_eq!(Brightness::new(1.5), Brightness::MAX);
        assert_eq!(Brightness::new(f32::NEG_INFINITY), Brightness::MIN);
        assert_eq!(Brightness::new(f32::INFINITY), Brightness::MAX);
    }

    #[test]
    fn nan_becomes_the_minimum_rather_than_travelling() {
        // `f32::clamp` passes NaN straight through, so this is the one case the
        // constructor has to intercept by hand.
        assert_eq!(Brightness::new(f32::NAN), Brightness::MIN);
        assert!(!Brightness::new(f32::NAN).fraction().is_nan());
    }

    #[test]
    fn percentages_round_trip() {
        for whole in 0..=100u8 {
            let level = Brightness::from_percent(f32::from(whole));
            assert_eq!(level.percent_rounded(), whole);
        }
    }

    #[test]
    fn percentages_outside_the_scale_are_clamped() {
        assert_eq!(Brightness::from_percent(-10.0), Brightness::MIN);
        assert_eq!(Brightness::from_percent(140.0), Brightness::MAX);
    }

    #[test]
    fn ddc_ranges_round_trip_at_both_common_maxima() {
        for max in [100u16, 255, 65535] {
            for raw in [0, max / 3, max / 2, max] {
                let level = Brightness::from_range(raw, max);
                assert_eq!(level.to_range(max), raw, "max {max}, raw {raw}");
            }
        }
    }

    #[test]
    fn a_zero_maximum_yields_the_minimum_rather_than_dividing_by_it() {
        assert_eq!(Brightness::from_range(0, 0), Brightness::MIN);
        assert_eq!(Brightness::from_range(50, 0), Brightness::MIN);
    }

    #[test]
    fn a_reading_above_the_reported_maximum_is_taken_as_the_maximum() {
        assert_eq!(Brightness::from_range(200, 100), Brightness::MAX);
    }

    #[test]
    fn stepping_saturates_at_both_ends() {
        assert_eq!(Brightness::MAX.stepped(0.1), Brightness::MAX);
        assert_eq!(Brightness::MIN.stepped(-0.1), Brightness::MIN);
        assert_eq!(
            Brightness::from_percent(50.0)
                .stepped(0.1)
                .percent_rounded(),
            60
        );
        assert_eq!(
            Brightness::from_percent(50.0)
                .stepped(-0.1)
                .percent_rounded(),
            40
        );
    }

    #[test]
    fn it_displays_as_a_whole_percentage() {
        assert_eq!(Brightness::from_percent(42.0).to_string(), "42%");
        assert_eq!(Brightness::MAX.to_string(), "100%");
    }
}
