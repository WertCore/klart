//! The private framework that dims the built-in panel.
//!
//! On Apple silicon this is the only thing that does. IOKit's
//! `IODisplaySetFloatParameter` is the documented interface for display
//! brightness, it is what every pre-2020 tutorial uses, and on these machines it
//! returns success and changes nothing.
//!
//! Nothing here is supported API. The framework is not on the dyld search path
//! and its headers are not published, so it is opened by full path at run time
//! and every symbol is optional: a macOS release that removes one produces an
//! error naming it rather than a process that will not start.

use std::ffi::{CString, c_void};
use std::sync::OnceLock;

/// The framework's full path. It is not on the search path, so a bare name will
/// not find it.
const FRAMEWORK: &str =
    "/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices";

/// The name used in errors and in `--json`.
pub(crate) const NAME: &str = "DisplayServices";

type GetBrightness = unsafe extern "C" fn(u32, *mut f32) -> i32;
type SetBrightness = unsafe extern "C" fn(u32, f32) -> i32;
type CanChangeBrightness = unsafe extern "C" fn(u32) -> bool;

/// The three entry points this crate uses, resolved once.
pub(crate) struct DisplayServices {
    get: GetBrightness,
    set: SetBrightness,
    can_change: CanChangeBrightness,
}

/// The framework, or [`None`] if it could not be opened or is missing a symbol.
///
/// Resolved once per process. Opening it is a file system hit and a link edit,
/// and a menu bar agent asks for brightness every time a menu opens.
pub(crate) fn display_services() -> Option<&'static DisplayServices> {
    static LOADED: OnceLock<Option<DisplayServices>> = OnceLock::new();
    LOADED.get_or_init(load).as_ref()
}

fn load() -> Option<DisplayServices> {
    let path = CString::new(FRAMEWORK).ok()?;

    // SAFETY: `path` is a live NUL-terminated string for the call. The handle is
    // deliberately never passed to `dlclose`: the function pointers below
    // outlive any one call, and unloading a framework other Apple code may also
    // hold open buys nothing.
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return None;
    }

    // SAFETY of each: the signatures are not published, so they are taken from
    // the ones every tool in this space has used against this framework for
    // years. A wrong one is undefined behaviour rather than a failed call, which
    // is why they are written out once, here, and nowhere else.
    Some(DisplayServices {
        get: unsafe { symbol(handle, "DisplayServicesGetBrightness") }?,
        set: unsafe { symbol(handle, "DisplayServicesSetBrightness") }?,
        can_change: unsafe { symbol(handle, "DisplayServicesCanChangeBrightness") }?,
    })
}

/// Resolves one symbol and reinterprets it as a function pointer.
///
/// # Safety
///
/// `T` must be the signature the symbol really has.
unsafe fn symbol<T: Copy>(handle: *mut c_void, name: &str) -> Option<T> {
    debug_assert_eq!(
        size_of::<T>(),
        size_of::<*mut c_void>(),
        "{name} was asked for as something that is not a plain function pointer"
    );

    let name = CString::new(name).ok()?;
    // SAFETY: `handle` came from `dlopen` and `name` is live across the call.
    let found = unsafe { libc::dlsym(handle, name.as_ptr()) };
    if found.is_null() {
        return None;
    }

    // SAFETY: the caller has promised `T` is the symbol's signature, and the
    // size assertion above rules out reading past the pointer.
    Some(unsafe { std::mem::transmute_copy(&found) })
}

impl DisplayServices {
    /// Whether the framework will change this display's brightness at all.
    ///
    /// The authority on the question. Asking it is cheaper than a failed set,
    /// and it is the only thing that distinguishes a panel this can drive from
    /// one it merely knows about.
    pub(crate) fn can_change(&self, display: u32) -> bool {
        // SAFETY: `display` is a `CGDirectDisplayID`; an identifier that is no
        // longer valid reads back as false rather than faulting.
        unsafe { (self.can_change)(display) }
    }

    /// Reads a display's level, or the framework's own status code.
    pub(crate) fn brightness(&self, display: u32) -> Result<f32, i32> {
        let mut level = 0.0_f32;

        // SAFETY: `level` is a live `f32` for the duration of the call.
        let status = unsafe { (self.get)(display, &raw mut level) };

        if status == 0 { Ok(level) } else { Err(status) }
    }

    /// Sets a display's level, or returns the framework's own status code.
    pub(crate) fn set_brightness(&self, display: u32, level: f32) -> Result<(), i32> {
        // SAFETY: both arguments are plain values.
        let status = unsafe { (self.set)(display, level) };

        if status == 0 { Ok(()) } else { Err(status) }
    }
}
