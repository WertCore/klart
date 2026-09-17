//! The two system interfaces display discovery is assembled from.
//!
//! Core Graphics owns the list of displays and their EDID numbers; the
//! IORegistry owns their names. Neither knows about the other, so [`graphics`]
//! and [`ioreg`] are read separately and joined in [`crate::display`].

pub(crate) mod av_service;
pub(crate) mod display_services;
pub(crate) mod graphics;
pub(crate) mod ioreg;
