//! The Linux end of a DDC/CI link.
//!
//! An `I2C_RDWR` ioctl on `/dev/i2c-N`. Everything above it — the framing, the
//! checksums, the reply validation, the retries — is [`crate::ddc`], shared with
//! every other platform, because DDC/CI is the same protocol everywhere and only
//! the transport is not. This file is the transport.
//!
//! One asymmetry with macOS is worth spelling out. `IOAVServiceWriteI2C` takes
//! the host's source address as a separate argument and puts it on the wire
//! itself. A raw I2C write does no such thing, so it is prepended here. Getting
//! that wrong produces a monitor that never answers and no error to explain it.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;

use crate::ddc::{CHIP_ADDRESS, HOST_ADDRESS, Link};

/// `I2C_RDWR`, from `linux/i2c-dev.h`.
const I2C_RDWR: libc::c_ulong = 0x0707;

/// `I2C_M_RD`, from `linux/i2c.h`.
const I2C_M_RD: u16 = 0x0001;

/// `struct i2c_msg`.
#[repr(C)]
struct Message {
    address: u16,
    flags: u16,
    length: u16,
    buffer: *mut u8,
}

/// `struct i2c_rdwr_ioctl_data`.
#[repr(C)]
struct Transaction {
    messages: *mut Message,
    count: u32,
}

/// A DDC/CI link over an I2C character device.
pub(crate) struct I2cLink {
    device: File,
}

impl I2cLink {
    /// Opens `/dev/i2c-{bus}`.
    ///
    /// Read and write, because DDC/CI needs both and a link that can only be
    /// read is no use. The usual reason this fails is permission: the node
    /// belongs to group `i2c` and a desktop user is not in it until a udev rule
    /// or an administrator says so.
    pub(crate) fn open(bus: u32) -> std::io::Result<Self> {
        Ok(Self {
            device: OpenOptions::new()
                .read(true)
                .write(true)
                .open(format!("/dev/i2c-{bus}"))?,
        })
    }

    /// Runs one transaction, returning the `errno` that stopped it.
    fn transfer(&self, messages: &mut [Message]) -> Result<(), i32> {
        let mut transaction = Transaction {
            messages: messages.as_mut_ptr(),
            count: u32::try_from(messages.len()).map_err(|_| libc::EINVAL)?,
        };

        // SAFETY: the file descriptor is open, and `transaction` points at a
        // live array of exactly `count` messages, each pointing at a live
        // buffer of its own stated length.
        let status = unsafe {
            libc::ioctl(
                self.device.as_raw_fd(),
                I2C_RDWR,
                std::ptr::addr_of_mut!(transaction),
            )
        };

        if status < 0 {
            return Err(std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO));
        }
        Ok(())
    }
}

impl Link for I2cLink {
    fn write(&self, bytes: &[u8]) -> Result<(), i32> {
        // The host's source address goes on the wire here, because a raw I2C
        // write has nowhere else to carry it. macOS's interface takes it as an
        // argument instead; the frame the monitor sees is identical.
        let mut framed = Vec::with_capacity(bytes.len() + 1);
        framed.push(HOST_ADDRESS);
        framed.extend_from_slice(bytes);

        let mut messages = [Message {
            address: u16::try_from(CHIP_ADDRESS).map_err(|_| libc::EINVAL)?,
            flags: 0,
            length: u16::try_from(framed.len()).map_err(|_| libc::EINVAL)?,
            buffer: framed.as_mut_ptr(),
        }];
        self.transfer(&mut messages)
    }

    fn read(&self, into: &mut [u8]) -> Result<(), i32> {
        let mut messages = [Message {
            address: u16::try_from(CHIP_ADDRESS).map_err(|_| libc::EINVAL)?,
            flags: I2C_M_RD,
            length: u16::try_from(into.len()).map_err(|_| libc::EINVAL)?,
            buffer: into.as_mut_ptr(),
        }];
        self.transfer(&mut messages)
    }
}
