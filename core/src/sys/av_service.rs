//! The private IOKit calls that carry I2C to an external monitor.
//!
//! `IOAVServiceReadI2C` and `IOAVServiceWriteI2C` are exported by IOKit and
//! documented nowhere. They are the only way to reach an external monitor's
//! DDC/CI link on Apple silicon: the Intel-era `IOFramebufferI2CRequest` has no
//! counterpart on these machines.
//!
//! IOKit is already linked — [`crate::sys::ioreg`] calls its supported registry
//! interface directly. These three are resolved through `dlsym` anyway, because
//! linking them would make a macOS release that withdraws one a binary that
//! cannot start rather than an error naming what is missing.

use std::ffi::{CString, c_void};
use std::sync::OnceLock;

use objc2_core_foundation::{CFRetained, CFType};

use crate::sys::ioreg::IoObject;

/// IOKit's full path. Already loaded in this process, so this resolves to the
/// handle that is open rather than mapping a second copy.
const FRAMEWORK: &str = "/System/Library/Frameworks/IOKit.framework/IOKit";

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "DDC/CI";

type CreateWithService = unsafe extern "C" fn(*const c_void, IoObject) -> *mut CFType;
type ReadI2C = unsafe extern "C" fn(*const CFType, u32, u32, *mut c_void, u32) -> i32;
type WriteI2C = unsafe extern "C" fn(*const CFType, u32, u32, *const c_void, u32) -> i32;

/// The three entry points this crate uses, resolved once.
struct AvKit {
    create: CreateWithService,
    read: ReadI2C,
    write: WriteI2C,
}

fn av_kit() -> Option<&'static AvKit> {
    static LOADED: OnceLock<Option<AvKit>> = OnceLock::new();
    LOADED.get_or_init(load).as_ref()
}

fn load() -> Option<AvKit> {
    let path = CString::new(FRAMEWORK).ok()?;

    // SAFETY: `path` is a live NUL-terminated string. The handle is never
    // closed, for the reasons given in `sys::display_services`.
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return None;
    }

    // SAFETY of each: these signatures are unpublished, so they are the ones
    // every tool in this space has used against these symbols for years. A wrong
    // one is undefined behaviour rather than a failed call, which is why they
    // are written out once, here.
    Some(AvKit {
        create: unsafe { symbol(handle, "IOAVServiceCreateWithService") }?,
        read: unsafe { symbol(handle, "IOAVServiceReadI2C") }?,
        write: unsafe { symbol(handle, "IOAVServiceWriteI2C") }?,
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

/// Whether these calls exist on this machine at all.
pub(crate) fn available() -> bool {
    av_kit().is_some()
}

/// An open I2C channel to one display.
pub(crate) struct AvService(CFRetained<CFType>);

impl AvService {
    /// Opens the channel a `DCPAVServiceProxy` represents.
    pub(crate) fn open(service: IoObject) -> Option<Self> {
        let kit = av_kit()?;

        // SAFETY: a null allocator is the default, and `service` is a live
        // registry object for the duration of the call. The result is a +1
        // reference, which `CFRetained::from_raw` takes over.
        let created = unsafe { (kit.create)(std::ptr::null(), service) };
        let created = std::ptr::NonNull::new(created)?;

        // SAFETY: non-null and owning a reference, which is what `from_raw`
        // documents.
        Some(Self(unsafe { CFRetained::from_raw(created) }))
    }

    /// Writes bytes onto the I2C bus, or returns the `IOReturn` that stopped it.
    pub(crate) fn write(&self, chip: u32, offset: u32, bytes: &[u8]) -> Result<(), i32> {
        let size = u32::try_from(bytes.len()).map_err(|_| -1)?;

        // SAFETY: `bytes` is live and `size` is its true length.
        let status = unsafe {
            (av_kit().ok_or(-1)?.write)(&*self.0, chip, offset, bytes.as_ptr().cast(), size)
        };

        if status == 0 { Ok(()) } else { Err(status) }
    }

    /// Fills a buffer from the I2C bus, or returns the `IOReturn` that stopped
    /// it.
    pub(crate) fn read(&self, chip: u32, offset: u32, into: &mut [u8]) -> Result<(), i32> {
        let size = u32::try_from(into.len()).map_err(|_| -1)?;

        // SAFETY: `into` is live and writable and `size` is its true length.
        let status = unsafe {
            (av_kit().ok_or(-1)?.read)(&*self.0, chip, offset, into.as_mut_ptr().cast(), size)
        };

        if status == 0 { Ok(()) } else { Err(status) }
    }
}
