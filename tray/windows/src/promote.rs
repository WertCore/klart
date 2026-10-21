//! Asking Windows to put the icon beside the clock rather than in the overflow.
//!
//! `Shell_NotifyIcon` has no flag for this. Since Windows 7 a new icon is added
//! to the overflow area by design and only the person is supposed to promote it
//! — Microsoft's own documentation says so, and there is no documented API that
//! overrides it.
//!
//! Windows 11 stores the choice per icon instead: `HKCU\Control
//! Panel\NotifyIconSettings` has a subkey for each registered icon, holding its
//! `ExecutablePath` and an `IsPromoted` DWORD. Explorer fills the subkey in when
//! the icon is added and reacts to `IsPromoted` immediately. Setting it when the
//! value is *absent* therefore means "the default, which is to be visible",
//! while a `0` is someone having hidden klart deliberately and is left alone.
//! This is undocumented and has been stable through Windows 11 22H2 to 25H2;
//! every failure here is silently ignored, because not being promoted is the
//! state the shell would have chosen anyway.
//!
//! Windows 10 has no such key, and its per-icon visibility lives in a binary
//! stream that is not worth overwriting behind Explorer's back. There the icon
//! stays in the overflow until it is dragged out, which is what that system asks
//! of every application.

use std::mem::size_of;
use std::ptr::{addr_of, addr_of_mut, null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_SZ, RegCloseKey, RegEnumKeyExW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

use crate::win::wide;

/// Where Windows 11 keeps the per-icon choice.
const SETTINGS: &str = r"Control Panel\NotifyIconSettings";

/// What one attempt found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// `IsPromoted` was absent and has now been set.
    Promoted,
    /// The person has already chosen, one way or the other. Nothing was written.
    Chosen,
    /// Windows 11's key is there, but Explorer has not written this icon's
    /// subkey yet. Worth another attempt shortly.
    Waiting,
    /// No per-icon settings: Windows 10, or a shell that does not use them.
    Unavailable,
}

/// Makes one attempt at promoting this executable's icon.
#[must_use]
pub fn ensure_visible() -> Outcome {
    let Ok(exe) = std::env::current_exe() else {
        return Outcome::Unavailable;
    };
    let ours = exe.to_string_lossy();

    let path = wide(SETTINGS);
    let mut settings: HKEY = null_mut();
    // SAFETY: `path` is live and NUL-terminated, and `settings` is a live out
    // parameter written on success.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            addr_of_mut!(settings),
        )
    };
    if opened != ERROR_SUCCESS {
        return Outcome::Unavailable;
    }

    let outcome = walk(settings, &ours);
    // SAFETY: `settings` was opened above and is closed exactly once.
    unsafe { RegCloseKey(settings) };

    outcome
}

/// Looks for this executable among the registered icons.
fn walk(settings: HKEY, ours: &str) -> Outcome {
    // The key exists, so this is the per-icon model whether or not our entry
    // has been written yet. With no subkeys at all, it is still worth retrying.
    let mut waiting = Outcome::Waiting;
    let mut index = 0;

    loop {
        let mut name = [0u16; 128];
        let mut length = u32::try_from(name.len()).unwrap_or(0);
        // SAFETY: the buffer and its length are live, and the trailing
        // parameters are optional (null).
        let status = unsafe {
            RegEnumKeyExW(
                settings,
                index,
                name.as_mut_ptr(),
                addr_of_mut!(length),
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };

        match status {
            ERROR_NO_MORE_ITEMS => return waiting,
            ERROR_MORE_DATA => {
                // A name longer than the buffer is not one of ours: the shell
                // names these with a decimal number.
                index += 1;
                continue;
            }
            ERROR_SUCCESS => {}
            // Access denied, or the key changed under us. There is nothing to
            // gain by guessing, and the icon simply stays where it is.
            _ => return waiting,
        }
        waiting = Outcome::Waiting;

        let mut subkey: HKEY = null_mut();
        // SAFETY: `name` holds a NUL-terminated string from the call above.
        let opened = unsafe {
            RegOpenKeyExW(
                settings,
                name.as_ptr(),
                0,
                KEY_READ | KEY_WRITE,
                addr_of_mut!(subkey),
            )
        };
        if opened != ERROR_SUCCESS {
            index += 1;
            continue;
        }

        let outcome = examine(subkey, ours);
        // SAFETY: `subkey` was opened above and is closed exactly once.
        unsafe { RegCloseKey(subkey) };

        match outcome {
            Outcome::Promoted | Outcome::Chosen => return outcome,
            Outcome::Waiting | Outcome::Unavailable => {}
        }

        index += 1;
    }
}

/// Checks one icon's subkey, promoting it if the person has not chosen.
fn examine(subkey: HKEY, ours: &str) -> Outcome {
    let Some(found) = string_value(subkey, "ExecutablePath") else {
        // Explorer writes the path in a moment; ours may still be an orphan.
        return Outcome::Waiting;
    };
    if !same_executable(&found, ours) {
        return Outcome::Waiting;
    }

    // Any value at all — 0 or 1 — is a choice made in Settings, and it wins.
    if dword_value(subkey, "IsPromoted").is_some() {
        return Outcome::Chosen;
    }

    let one: u32 = 1;
    let name = wide("IsPromoted");
    // SAFETY: the name is NUL-terminated and `one` is a live DWORD of the size
    // given.
    let written = unsafe {
        RegSetValueExW(
            subkey,
            name.as_ptr(),
            0,
            REG_DWORD,
            addr_of!(one).cast(),
            u32::try_from(size_of::<u32>()).unwrap_or(4),
        )
    };

    if written == ERROR_SUCCESS {
        Outcome::Promoted
    } else {
        // The subkey is there but cannot be written — Explorer may be holding
        // it. The icon is no worse off than before; the next tick tries again.
        Outcome::Waiting
    }
}

/// A string value, without its terminator.
fn string_value(key: HKEY, name: &str) -> Option<String> {
    let value = wide(name);
    let mut kind = 0;
    let mut bytes = [0u8; 1024];
    let mut size = u32::try_from(bytes.len()).unwrap_or(0);

    // SAFETY: the name is live and NUL-terminated, the buffer and size are live
    // out parameters for the size given, and the buffer is at least `size`
    // bytes.
    let status = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            null(),
            addr_of_mut!(kind),
            bytes.as_mut_ptr(),
            addr_of_mut!(size),
        )
    };
    if status != ERROR_SUCCESS || kind != REG_SZ {
        return None;
    }

    // The size is in bytes and may not include a terminator; both are handled
    // by stopping at the first zero and at the end.
    let units = usize::try_from(size / 2).unwrap_or(0).min(bytes.len() / 2);
    let wide: Vec<u16> = bytes[..units * 2]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();

    Some(String::from_utf16_lossy(&wide))
}

fn dword_value(key: HKEY, name: &str) -> Option<u32> {
    let value = wide(name);
    let mut kind = 0;
    let mut data: u32 = 0;
    let mut size = u32::try_from(size_of::<u32>()).unwrap_or(4);

    // SAFETY: as `string_value`, with a buffer of the type asked for.
    let status = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            null(),
            addr_of_mut!(kind),
            addr_of_mut!(data).cast(),
            addr_of_mut!(size),
        )
    };

    (status == ERROR_SUCCESS && kind == REG_DWORD).then_some(data)
}

/// Whether a registered icon's executable is this one.
///
/// The shell writes the path it was given, which can differ in case, in
/// quoting, or in having a `\\?\` prefix; none of those are a different program.
fn same_executable(found: &str, ours: &str) -> bool {
    normalize(found).eq_ignore_ascii_case(normalize(ours))
}

fn normalize(path: &str) -> &str {
    let trimmed = path.trim().trim_matches('"');
    trimmed.strip_prefix(r"\\?\").unwrap_or(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_path_in_different_clothes_is_the_same_program() {
        let ours = r"C:\Program Files\klart\klart-tray.exe";

        assert!(same_executable(ours, ours));
        assert!(same_executable(
            r"c:\program files\KLART\klart-tray.exe",
            ours
        ));
        assert!(same_executable(
            r#""C:\Program Files\klart\klart-tray.exe""#,
            ours
        ));
        assert!(same_executable(
            r"\\?\C:\Program Files\klart\klart-tray.exe",
            ours
        ));
    }

    #[test]
    fn another_executable_in_the_same_folder_is_not_ours() {
        assert!(!same_executable(
            r"C:\Program Files\klart\klart.exe",
            r"C:\Program Files\klart\klart-tray.exe"
        ));
        assert!(!same_executable("", r"C:\klart-tray.exe"));
    }
}
