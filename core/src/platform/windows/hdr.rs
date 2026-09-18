//! Whether a display is in HDR.
//!
//! Worth knowing because HDR takes brightness control away and does not say so.
//! A monitor switched into an HDR picture mode may pin its brightness to a
//! preset, weaken what a DDC/CI write does, or refuse hardware brightness
//! control outright — and the gamma ramp is no refuge either, because Windows
//! does not guarantee ramp behaviour in HDR mode. From the outside all of that
//! looks like a brightness control that has stopped working, which is how it
//! gets reported.
//!
//! The query is a different family from everything else this platform module
//! does: not `EnumDisplayMonitors` and an `HMONITOR`, but the display
//! configuration API, which addresses a monitor by adapter LUID and target id.
//! There is no call that turns one into the other, so the join is the monitor's
//! device path — the same identifier [`super::monitors`] already joins the
//! geometry and the registry on.
//!
//! `DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO` is marked deprecated in
//! recent SDKs in favour of a wider `_2` form that distinguishes HDR from wide
//! colour gamut. It is still answered, and the distinction does not matter here:
//! the question is only whether the display is in a mode that takes brightness
//! control away.

use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;

use super::monitors::{instance_path, wide_to_string};

/// The bit that says the display is in advanced colour, which is what the
/// control panel calls HDR.
///
/// Bit 0 of the same word is `advancedColorSupported`, which is a capability
/// rather than a state and is not what is being asked here — a monitor that
/// supports HDR and is not in it behaves like any other monitor.
const ADVANCED_COLOUR_ENABLED: u32 = 1 << 1;

/// Whether this display has HDR switched on.
///
/// [`None`] when it could not be determined — no path matched, or the
/// configuration could not be read at all. A caller must treat that as "not
/// known" rather than as "no": saying a display is not in HDR on the strength of
/// a failed query would put the blame for a refused write in the wrong place.
pub(super) fn enabled_for(instance: &str) -> Option<bool> {
    for path in active_paths()? {
        let target = path.targetInfo;

        let Some(found) = device_path(target.adapterId, target.id) else {
            continue;
        };
        if instance_path(&found).as_deref() != Some(instance) {
            continue;
        }

        let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
            header: header(
                DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>(),
                target.adapterId,
                target.id,
            ),
            ..Default::default()
        };

        // SAFETY: the header names this struct's own type and size, which is
        // what the call dispatches on, and the struct outlives the call.
        if unsafe { DisplayConfigGetDeviceInfo(std::ptr::from_mut(&mut info).cast()) } != 0 {
            return None;
        }

        // SAFETY: the union's two arms are a `u32` of bitfields and a `u32`
        // named `value`, which is what it exists for.
        let bits = unsafe { info.Anonymous.value };
        return Some(bits & ADVANCED_COLOUR_ENABLED != 0);
    }

    None
}

/// Every display path currently driving a monitor.
fn active_paths() -> Option<Vec<DISPLAYCONFIG_PATH_INFO>> {
    let mut paths = 0u32;
    let mut modes = 0u32;

    // SAFETY: both counts are live and writable for the call.
    let sized =
        unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) };
    if sized != ERROR_SUCCESS {
        return None;
    }

    let mut path_buffer = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
    let mut mode_buffer = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];

    // SAFETY: both buffers hold at least the counts just asked for, and both
    // counts are writable so the call can report how many it filled.
    let queried = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths,
            path_buffer.as_mut_ptr(),
            &mut modes,
            mode_buffer.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    if queried != ERROR_SUCCESS {
        return None;
    }

    // The call may report fewer than were asked for, and the tail is whatever
    // `default` left there rather than a path.
    path_buffer.truncate(paths as usize);
    Some(path_buffer)
}

/// The device path of whatever is on one target, such as
/// `\\?\DISPLAY#SAM71E3#5&1a2b3c4d&0&UID256#{e6f07b5f-...}`.
fn device_path(adapter: windows_sys::Win32::Foundation::LUID, target: u32) -> Option<String> {
    let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: header(
            DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
            adapter,
            target,
        ),
        ..Default::default()
    };

    // SAFETY: as above — the header names this struct's type and size.
    if unsafe { DisplayConfigGetDeviceInfo(std::ptr::from_mut(&mut name).cast()) } != 0 {
        return None;
    }

    Some(wide_to_string(&name.monitorDevicePath))
}

/// The header every display configuration request begins with.
///
/// The call has no other way to know what was asked: it dispatches on the type
/// and trusts the size, so a wrong size is a buffer overrun rather than an
/// error. Built in one place for that reason.
fn header(
    request: i32,
    size: usize,
    adapter: windows_sys::Win32::Foundation::LUID,
    target: u32,
) -> windows_sys::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_HEADER {
    windows_sys::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: request,
        size: u32::try_from(size).unwrap_or(0),
        adapterId: adapter,
        id: target,
    }
}
