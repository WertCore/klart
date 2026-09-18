//! Starting with the session, the way Windows does it.
//!
//! A value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, holding
//! the path of the executable to start. The oldest and least surprising of the
//! several mechanisms Windows offers, it needs no elevation, and it is the one
//! Task Manager's Startup tab shows — so a person who wants it gone can find it
//! without being told where to look.

use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

use crate::autostart::LoginItem;

use super::monitors::to_wide;

/// Where Windows looks at login.
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// What the value is called there.
const VALUE: &str = "klart";

/// Whether the agent will start with the session.
pub fn status() -> LoginItem {
    let Some(key) = open(KEY_READ) else {
        return LoginItem::Unavailable;
    };

    let name = to_wide(VALUE);
    let mut length: u32 = 0;

    // SAFETY: a null data pointer asks only for the length, which is the
    // documented way to test for a value's existence.
    let found = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(length),
        )
    };

    // SAFETY: opened above, closed exactly once.
    unsafe { RegCloseKey(key) };

    if found == 0 {
        LoginItem::Enabled
    } else {
        LoginItem::Disabled
    }
}

/// Writes or removes the value.
///
/// # Errors
///
/// The Windows error code, which is the only detail this API offers.
pub fn set(enabled: bool) -> Result<LoginItem, String> {
    let key = open(KEY_WRITE | KEY_READ)
        .ok_or_else(|| "could not open the Run key under HKEY_CURRENT_USER".to_owned())?;

    let name = to_wide(VALUE);

    let outcome = if enabled {
        let executable = std::env::current_exe()
            .map_err(|problem| problem.to_string())?
            .display()
            .to_string();

        // Quoted, because a path with a space in it — which `C:\Program Files`
        // guarantees — is otherwise read as a command and its arguments.
        let value = to_wide(&format!("\"{executable}\""));
        let bytes = u32::try_from(std::mem::size_of_val(value.as_slice())).unwrap_or(0);

        // SAFETY: `value` is a live NUL-terminated wide string and `bytes` is
        // its length including the terminator, which is what `REG_SZ` wants.
        unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr().cast(), bytes) }
    } else {
        // SAFETY: `name` is live and NUL terminated.
        let deleted = unsafe { RegDeleteValueW(key, name.as_ptr()) };
        // Not being there is the state that was asked for.
        if deleted == 2 { 0 } else { deleted }
    };

    // SAFETY: opened above, closed exactly once.
    unsafe { RegCloseKey(key) };

    if outcome != 0 {
        return Err(format!("the registry refused the change: error {outcome}"));
    }
    Ok(status())
}

fn open(access: u32) -> Option<HKEY> {
    let path = to_wide(RUN);
    let mut key: HKEY = std::ptr::null_mut();

    // SAFETY: `path` is live and NUL terminated, and `key` is a live out
    // parameter that every caller closes.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            access,
            std::ptr::addr_of_mut!(key),
        )
    };

    (opened == 0).then_some(key)
}
