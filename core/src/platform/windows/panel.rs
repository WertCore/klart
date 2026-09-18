//! The machine's own panel, through WMI.
//!
//! A laptop screen has no DDC/CI — there is no I2C bus to a panel soldered to
//! the lid — and the interface Windows offers instead is a WMI class,
//! `WmiMonitorBrightnessMethods`, in the `root\WMI` namespace. The older
//! `IOCTL_VIDEO_SET_DISPLAY_BRIGHTNESS` has been obsolete since Vista and
//! Microsoft's own documentation points here.
//!
//! That means COM, which is why this is the longest file on this platform for
//! the least interesting mechanism.
//!
//! # Aiming it
//!
//! WMI's brightness applies to *a* monitor and it is not necessarily the one
//! being asked about. Every instance carries an `InstanceName` built from the
//! device instance path — `DISPLAY\SAM71E3\5&...&UID256_0` — so it is matched
//! against the path [`super::monitors`] already computes rather than taking
//! whichever instance WMI happens to list first. Without that, dimming an
//! external monitor would dim the laptop screen.

use std::sync::OnceLock;

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4, VT_UI1, VT_UI4};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY, WbemLocator,
};
use windows::core::BSTR;

use crate::Brightness;
use crate::backend::Backend;
use crate::error::{Error, Result};

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "WMI";

/// How long the panel is given to reach a new level, in seconds.
///
/// `WmiSetBrightness` takes a timeout and returns once it has elapsed or the
/// change has happened. Zero, because this is a slider: waiting a second for
/// each step of a drag would make it unusable, and the panel gets there anyway.
const TIMEOUT: u32 = 0;

/// The panel, and the WMI connection that reaches it.
pub(crate) struct Wmi {
    services: IWbemServices,
    /// The `InstanceName` this backend is bound to.
    instance: String,
    display: String,
}

impl Wmi {
    /// Binds to the panel whose WMI instance matches a device instance path.
    ///
    /// # Errors
    ///
    /// [`Error::CannotReach`] when WMI is unavailable or lists no brightness
    /// instance for this display, which is the ordinary answer for every
    /// external monitor.
    pub(crate) fn open(device_instance: &str, display: &str) -> Result<Self> {
        let cannot_reach = || Error::CannotReach {
            mechanism: NAME,
            display: display.to_owned(),
        };

        let services = connect().ok_or_else(cannot_reach)?.clone();

        // WMI's instance name is the device instance path with a suffix, so the
        // match is on the prefix. Comparison is case-insensitive because the
        // registry and WMI do not agree on the case of these strings.
        let wanted = device_instance.to_ascii_uppercase();
        let instance = instances(&services, "WmiMonitorBrightness")
            .into_iter()
            .find(|found| found.to_ascii_uppercase().starts_with(&wanted))
            .ok_or_else(cannot_reach)?;

        Ok(Self {
            services,
            instance,
            display: display.to_owned(),
        })
    }
}

impl Backend for Wmi {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self) -> Result<Brightness> {
        let query = format!(
            "SELECT * FROM WmiMonitorBrightness WHERE InstanceName='{}'",
            escape(&self.instance)
        );

        let object = first(&self.services, &query).ok_or_else(|| Error::NoReply {
            mechanism: NAME,
            display: self.display.clone(),
            attempts: 1,
        })?;

        // Already a percentage. Unlike DDC/CI and sysfs, WMI normalises it, so
        // there is no per-panel maximum to scale against.
        let percent = property(&object, "CurrentBrightness").ok_or_else(|| Error::NoReply {
            mechanism: NAME,
            display: self.display.clone(),
            attempts: 1,
        })?;

        Ok(Brightness::from_percent(percent as f32))
    }

    fn set(&self, level: Brightness) -> Result<()> {
        let path = format!(
            "WmiMonitorBrightnessMethods.InstanceName='{}'",
            escape(&self.instance)
        );

        let failed = || Error::MechanismFailed {
            mechanism: NAME,
            call: "WmiSetBrightness",
            code: -1,
        };

        // SAFETY: every argument below is live for the call, and each COM
        // pointer came from the call above it.
        unsafe {
            // The method's input signature, which is the only way to build the
            // parameter object it expects.
            let mut class: Option<IWbemClassObject> = None;
            self.services
                .GetObject(
                    &BSTR::from("WmiMonitorBrightnessMethods"),
                    Default::default(),
                    None,
                    Some(std::ptr::addr_of_mut!(class)),
                    None,
                )
                .map_err(|_| failed())?;

            let mut signature: Option<IWbemClassObject> = None;
            class
                .ok_or_else(failed)?
                .GetMethod(
                    &windows::core::HSTRING::from("WmiSetBrightness"),
                    0,
                    &mut signature,
                    std::ptr::null_mut(),
                )
                .map_err(|_| failed())?;

            let parameters = signature
                .ok_or_else(failed)?
                .SpawnInstance(0)
                .map_err(|_| failed())?;

            put(&parameters, "Timeout", VARIANT::from(TIMEOUT))?;
            put(
                &parameters,
                "Brightness",
                VARIANT::from(u32::from(level.percent_rounded())),
            )?;

            self.services
                .ExecMethod(
                    &BSTR::from(path),
                    &BSTR::from("WmiSetBrightness"),
                    Default::default(),
                    None,
                    &parameters,
                    None,
                    None,
                )
                .map_err(|_| failed())?;
        }

        Ok(())
    }
}

/// The `root\WMI` namespace, connected once per process.
///
/// Held for the life of the process rather than opened per call: connecting is
/// several round trips through COM, and a slider asks for brightness many times
/// a second.
fn connect() -> Option<&'static IWbemServices> {
    static SERVICES: OnceLock<Option<Connection>> = OnceLock::new();
    SERVICES.get_or_init(open_namespace).as_ref().map(|c| &c.0)
}

/// A wrapper so the connection can be held in a `static`.
///
/// `IWbemServices` is a COM pointer, which is reference counted rather than
/// thread-bound; the proxy is configured below to be callable from any thread.
struct Connection(IWbemServices);

// SAFETY: the interface is a multi-threaded-apartment proxy — COM is
// initialised with `COINIT_MULTITHREADED` and the blanket is set below — so
// calls from any thread are marshalled rather than rejected.
unsafe impl Send for Connection {}
unsafe impl Sync for Connection {}

fn open_namespace() -> Option<Connection> {
    // SAFETY: each call's arguments are live, and each pointer came from the
    // call before it. COM is never uninitialised: the connection above lives as
    // long as the process, and tearing it down at exit buys nothing.
    unsafe {
        // `S_FALSE` means this thread was already initialised, which is success
        // for the purpose here. Only a genuine failure is worth stopping for.
        let initialised = CoInitializeEx(None, COINIT_MULTITHREADED);
        if initialised.is_err() {
            return None;
        }

        let locator: IWbemLocator =
            CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;

        let services = locator
            .ConnectServer(
                &BSTR::from(r"root\WMI"),
                &BSTR::new(),
                &BSTR::new(),
                &BSTR::new(),
                0,
                &BSTR::new(),
                None,
            )
            .ok()?;

        // Without this the proxy refuses calls: WMI requires the client to say
        // what authentication it will accept before it will answer anything.
        CoSetProxyBlanket(
            &services,
            u32::MAX, // RPC_C_AUTHN_DEFAULT
            u32::MAX, // RPC_C_AUTHZ_DEFAULT
            None,
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )
        .ok()?;

        Some(Connection(services))
    }
}

/// Every `InstanceName` a class publishes.
fn instances(services: &IWbemServices, class: &str) -> Vec<String> {
    let mut found = Vec::new();
    let query = format!("SELECT InstanceName FROM {class}");

    // SAFETY: the enumerator and every object come from the calls above them,
    // and each is released when its wrapper drops.
    unsafe {
        let Ok(enumerator) = services.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(query),
            // `WBEM_FLAG_RETURN_WHEN_COMPLETE` is zero in the SDK, so the
            // binding omits it and forward-only is the whole flag.
            WBEM_FLAG_FORWARD_ONLY,
            None,
        ) else {
            return found;
        };

        loop {
            let mut object = [const { None }; 1];
            let mut returned = 0_u32;

            if enumerator.Next(-1, &mut object, &mut returned).is_err() || returned == 0 {
                break;
            }

            let Some(object) = object[0].take() else {
                break;
            };
            if let Some(name) = text(&object, "InstanceName") {
                found.push(name);
            }
        }
    }
    found
}

/// The first object a query returns, if it returns one.
fn first(services: &IWbemServices, query: &str) -> Option<IWbemClassObject> {
    // SAFETY: as `instances`.
    unsafe {
        let enumerator = services
            .ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from(query),
                WBEM_FLAG_FORWARD_ONLY,
                None,
            )
            .ok()?;

        let mut object = [const { None }; 1];
        let mut returned = 0_u32;
        if enumerator.Next(-1, &mut object, &mut returned).is_err() || returned == 0 {
            return None;
        }

        object[0].take()
    }
}

/// Writes one input parameter.
fn put(parameters: &IWbemClassObject, name: &str, value: VARIANT) -> Result<()> {
    // SAFETY: `name` and `value` are live across the call.
    unsafe {
        parameters
            .Put(&windows::core::HSTRING::from(name), 0, &value, 0)
            .map_err(|_| Error::MechanismFailed {
                mechanism: NAME,
                call: "IWbemClassObject::Put",
                code: -1,
            })
    }
}

/// Reads a whole-number property.
fn property(object: &IWbemClassObject, name: &str) -> Option<u32> {
    let value = read(object, name)?;
    number(&value)
}

/// Reads a string property.
fn text(object: &IWbemClassObject, name: &str) -> Option<String> {
    let value = read(object, name)?;

    // SAFETY: the tag is checked before the union is read, which is the only
    // thing that makes reading it meaningful.
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        if inner.vt != VT_BSTR {
            return None;
        }
        Some(inner.Anonymous.bstrVal.to_string())
    }
}

fn read(object: &IWbemClassObject, name: &str) -> Option<VARIANT> {
    let mut value = VARIANT::default();

    // SAFETY: `value` is a live `VARIANT` this call initialises.
    unsafe {
        object
            .Get(
                &windows::core::HSTRING::from(name),
                0,
                &mut value,
                None,
                None,
            )
            .ok()?;
    }
    Some(value)
}

/// Reads a whole number out of a `VARIANT`, whatever width WMI chose.
///
/// The union is only meaningful for the tag in `vt`. WMI documents
/// `CurrentBrightness` as a byte, and returns it as one on every machine anyone
/// has reported — but a provider is free to widen it, and reading the wrong arm
/// of a union is reading whatever else was in those bytes.
fn number(value: &VARIANT) -> Option<u32> {
    // SAFETY: the tag is checked before the union is read.
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        match inner.vt {
            VT_UI1 => Some(u32::from(inner.Anonymous.bVal)),
            VT_UI4 => Some(inner.Anonymous.ulVal),
            VT_I4 => u32::try_from(inner.Anonymous.lVal).ok(),
            _ => None,
        }
    }
}

/// Makes a string safe to embed in a WQL literal.
///
/// Instance names contain backslashes and ampersands, and WQL takes a backslash
/// as an escape — so an unescaped one silently changes which instance is
/// matched, or matches none.
fn escape(instance: &str) -> String {
    instance.replace('\\', r"\\").replace('\'', r"\'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instance_name_is_escaped_for_the_query_it_goes_into() {
        // The real shape of one of these. The backslashes are what matter: WQL
        // reads a lone backslash as an escape, so this would otherwise match
        // nothing and the panel would look unreachable.
        assert_eq!(
            escape(r"DISPLAY\SAM71E3\5&1a2b3c4d&0&UID256_0"),
            r"DISPLAY\\SAM71E3\\5&1a2b3c4d&0&UID256_0"
        );
    }

    #[test]
    fn a_quote_cannot_close_the_literal_early() {
        assert_eq!(escape("it's"), r"it\'s");
    }
}
