//! Finding monitors, and the EDID the driver kept for each.
//!
//! Two enumerations that have to be joined, because Windows keeps the geometry
//! and the display's own description in different places and gives them no
//! common identifier.
//!
//! `EnumDisplayMonitors` yields an `HMONITOR` per monitor, which is what every
//! brightness call takes, together with where it sits on the desktop.
//! `EnumDisplayDevices` turns that into a device instance path, and the EDID
//! lives under that path in the registry, exactly as the driver stored it when
//! the monitor was first plugged in.
//!
//! The join is the device instance path, which both sides agree on. That is the
//! same shape of problem macOS has — Core Graphics and the IORegistry share no
//! identifier either — and it is solved the same way: read both, match on
//! something the display itself published.

use windows_sys::Win32::Foundation::{LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITORINFOEXW,
};
use windows_sys::Win32::System::Registry::{
    HKEY, KEY_READ, REG_VALUE_TYPE, RegCloseKey, RegQueryValueExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EDD_GET_DEVICE_INTERFACE_NAME, MONITORINFOF_PRIMARY,
};
use windows_sys::core::BOOL;

/// One monitor as Windows sees it.
pub(crate) struct Monitor {
    /// The handle every brightness call takes.
    pub handle: HMONITOR,
    /// Where it sits on the virtual desktop, in pixels.
    pub rect: RECT,
    /// Whether it is the primary monitor.
    pub primary: bool,
    /// The adapter device name, such as `\\.\DISPLAY1`.
    pub adapter: String,
    /// The raw EDID the driver stored, when it can be found.
    pub edid: Vec<u8>,
    /// The device instance path, such as `DISPLAY\SAM71E3\5&1a2b3c4d&0&UID256`.
    ///
    /// The registry keeps the EDID under it, and WMI names its brightness
    /// instances after it — which is what lets a brightness call be aimed at one
    /// particular panel rather than at whichever one WMI lists first.
    pub instance: Option<String>,
}

/// Every monitor attached to the machine.
pub(crate) fn attached() -> Vec<Monitor> {
    let mut handles: Vec<HMONITOR> = Vec::new();

    // SAFETY: a null device context and rectangle mean "every monitor", and the
    // callback below matches the signature `MONITORENUMPROC` requires. The
    // pointer handed through `LPARAM` is the `Vec` above, which outlives the
    // call because the call returns before this function does.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            std::ptr::addr_of_mut!(handles) as LPARAM,
        );
    }

    handles.into_iter().filter_map(describe).collect()
}

unsafe extern "system" fn collect(
    handle: HMONITOR,
    _context: HDC,
    _rect: *mut RECT,
    into: LPARAM,
) -> BOOL {
    // SAFETY: `into` is the `Vec` `attached` passed, and this callback only runs
    // during that call.
    let handles = unsafe { &mut *(into as *mut Vec<HMONITOR>) };
    handles.push(handle);
    TRUE
}

fn describe(handle: HMONITOR) -> Option<Monitor> {
    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = u32::try_from(size_of::<MONITORINFOEXW>()).ok()?;

    // SAFETY: `info` is a live `MONITORINFOEXW` whose `cbSize` says so, which is
    // how this call knows which of the two structures it was given.
    if unsafe { GetMonitorInfoW(handle, std::ptr::addr_of_mut!(info).cast()) } == 0 {
        return None;
    }

    let adapter = wide_to_string(&info.szDevice);

    let instance = instance_for(&adapter);

    Some(Monitor {
        handle,
        rect: info.monitorInfo.rcMonitor,
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        edid: instance
            .as_deref()
            .and_then(|path| read_edid(&registry_path(path)))
            .unwrap_or_default(),
        instance,
        adapter,
    })
}

/// The device instance path for whatever is plugged into an adapter.
///
/// `EnumDisplayDevicesW` on the adapter yields the monitor's device interface
/// name; the instance path is the same identifiers with different punctuation.
fn instance_for(adapter: &str) -> Option<String> {
    let mut device: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
    device.cb = u32::try_from(size_of::<DISPLAY_DEVICEW>()).ok()?;

    let adapter_wide = to_wide(adapter);

    // SAFETY: `adapter_wide` is NUL terminated and lives across the call, and
    // `device` has its `cb` set, which is how the call knows its size.
    let found = unsafe {
        EnumDisplayDevicesW(
            adapter_wide.as_ptr(),
            0,
            std::ptr::addr_of_mut!(device),
            EDD_GET_DEVICE_INTERFACE_NAME,
        )
    };
    if found == 0 {
        return None;
    }

    instance_path(&wide_to_string(&device.DeviceID))
}

/// Turns a device interface name into a device instance path.
///
/// `EnumDisplayDevicesW` hands back something like
/// `\\?\DISPLAY#SAM71E3#5&...#{e6f07b5f-...}`. Everything else wants
/// `DISPLAY\SAM71E3\5&...` — the same identifiers, different punctuation, and
/// without the device interface class at the end.
fn instance_path(interface: &str) -> Option<String> {
    let trimmed = interface.strip_prefix(r"\\?\")?;
    // The trailing brace is the device interface class, which is not part of the
    // instance path.
    let without_class = trimmed.split('#').take(3).collect::<Vec<_>>();
    if without_class.len() < 3 {
        return None;
    }
    Some(without_class.join("\\"))
}

/// Where the driver stored a device's EDID.
fn registry_path(instance: &str) -> String {
    format!(r"SYSTEM\CurrentControlSet\Enum\{instance}\Device Parameters")
}

fn read_edid(path: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RegOpenKeyExW};

    let path_wide = to_wide(path);
    let mut key: HKEY = std::ptr::null_mut();

    // SAFETY: `path_wide` is NUL terminated and live, and `key` is a live out
    // parameter closed below on every path that opens it.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path_wide.as_ptr(),
            0,
            KEY_READ,
            std::ptr::addr_of_mut!(key),
        )
    };
    if opened != 0 {
        return None;
    }

    let name = to_wide("EDID");
    let mut kind: REG_VALUE_TYPE = 0;
    let mut length: u32 = 0;

    // SAFETY: a null data pointer asks only for the length, which is what the
    // documented two-call pattern for this function requires.
    let sized = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::addr_of_mut!(kind),
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(length),
        )
    };

    let mut buffer = vec![0_u8; length as usize];
    if sized == 0 && length > 0 {
        // SAFETY: `buffer` is exactly `length` bytes, which is what the call
        // above reported and what this one is told.
        let read = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::addr_of_mut!(kind),
                buffer.as_mut_ptr(),
                std::ptr::addr_of_mut!(length),
            )
        };
        if read != 0 {
            buffer.clear();
        }
    } else {
        buffer.clear();
    }

    // SAFETY: `key` was opened above and is closed exactly once, here.
    unsafe { RegCloseKey(key) };

    (!buffer.is_empty()).then_some(buffer)
}

/// A NUL-terminated wide string, for the `W` half of the API.
pub(crate) fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The other direction, stopping at the first NUL.
pub(crate) fn wide_to_string(wide: &[u16]) -> String {
    let end = wide
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_round_trip_and_stop_at_the_terminator() {
        let wide = to_wide(r"\\.\DISPLAY1");
        assert_eq!(wide_to_string(&wide), r"\\.\DISPLAY1");

        // Windows hands back fixed-size arrays padded with NULs, so everything
        // after the first one is not part of the name.
        let padded = [0x41_u16, 0x42, 0x00, 0x43, 0x44];
        assert_eq!(wide_to_string(&padded), "AB");
    }

    #[test]
    fn an_interface_name_becomes_the_instance_path_everything_else_wants() {
        let interface =
            r"\\?\DISPLAY#SAM71E3#5&1a2b3c4d&0&UID256#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

        assert_eq!(
            instance_path(interface).as_deref(),
            Some(r"DISPLAY\SAM71E3\5&1a2b3c4d&0&UID256")
        );
        assert_eq!(
            registry_path(r"DISPLAY\SAM71E3\5&1a2b3c4d&0&UID256"),
            r"SYSTEM\CurrentControlSet\Enum\DISPLAY\SAM71E3\5&1a2b3c4d&0&UID256\Device Parameters"
        );
    }

    #[test]
    fn something_that_is_not_an_interface_name_yields_no_path() {
        assert_eq!(instance_path("DISPLAY1"), None);
        assert_eq!(instance_path(r"\\?\DISPLAY#SAM71E3"), None, "too few parts");
    }
}
