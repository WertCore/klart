//! Brightness writes, away from the message loop.
//!
//! A DDC/CI write takes about two hundred milliseconds on the monitor this was
//! written against — the call waits for the display, and the display does not
//! answer at pointer speed. Doing that inside `WM_MOUSEMOVE` would stop the
//! message loop for every step of a drag, so the knob would move in fifth-of-a-
//! second jumps however often the mouse was sampled. The panel therefore hands
//! values to this thread and repaints immediately; what the person sees follows
//! the pointer, and what the display does follows as fast as the display can.
//!
//! The thread owns its own [`Control`]s rather than borrowing the driver's. A
//! backend is deliberately not `Send` — `Wmi` holds a COM proxy that belongs to
//! the apartment that opened it — so the controls are enumerated again here, on
//! the thread that will use them. The driver keeps its own set for reading, and
//! the two meet only in the indices the panel and the driver agree on.
//!
//! Values are coalesced rather than queued. While one display is being written,
//! later values pile up in the channel; when the write returns, only the newest
//! per display is sent, because the intermediate ones describe positions the
//! pointer has already left.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::{self, JoinHandle};

use klart_core::{Brightness, Control, controls};

/// The writes, and the thread that makes them.
pub struct Writer {
    sender: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

enum Message {
    /// Put one display at a level.
    Set { display: usize, percent: u8 },
    /// Enumerate the displays again, because the set may have changed.
    Refresh,
    /// Stop, releasing the COM apartment on the thread that made it.
    Stop,
}

impl Writer {
    /// Starts the thread.
    ///
    /// [`None`] when the thread could not be created, which leaves the agent to
    /// write on the message thread as it did before: slower, but working.
    pub fn start() -> Option<Self> {
        let (sender, receiver) = channel();
        let thread = thread::Builder::new()
            .name("klart-brightness".to_owned())
            .spawn(move || run(receiver))
            .ok()?;

        Some(Self {
            sender,
            thread: Some(thread),
        })
    }

    /// Asks for a level. Returns before the display has been written to.
    pub fn set(&self, display: usize, percent: u8) {
        // The receiver only goes away when the thread is gone, and the thread is
        // joined before this struct is dropped.
        let _ = self.sender.send(Message::Set { display, percent });
    }

    /// Asks the thread to enumerate the displays again.
    pub fn refresh(&self) {
        let _ = self.sender.send(Message::Refresh);
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            // The wait is one write long at most, and it wins the COM objects
            // being released on their own thread before the process exits.
            let _ = thread.join();
        }
    }
}

fn run(receiver: Receiver<Message>) {
    let mut displays = controls().unwrap_or_default();
    let mut pending: Vec<Option<u8>> = vec![None; displays.len()];

    loop {
        let message = match receiver.recv() {
            Ok(message) => message,
            Err(_) => return,
        };

        match message {
            Message::Stop => return,
            Message::Refresh => {
                displays = controls().unwrap_or_default();
                pending = vec![None; displays.len()];
            }
            Message::Set { display, percent } => {
                if let Some(slot) = pending.get_mut(display) {
                    *slot = Some(percent);
                }
                if !drain(&receiver, &mut displays, &mut pending) {
                    return;
                }
                write(&displays, &mut pending);
            }
        }
    }
}

/// Takes everything already queued without waiting.
///
/// `false` when the thread has been told to stop or the sender is gone.
fn drain(
    receiver: &Receiver<Message>,
    displays: &mut Vec<Control>,
    pending: &mut Vec<Option<u8>>,
) -> bool {
    loop {
        match receiver.try_recv() {
            Ok(Message::Set { display, percent }) => {
                if let Some(slot) = pending.get_mut(display) {
                    *slot = Some(percent);
                }
            }
            Ok(Message::Refresh) => {
                *displays = controls().unwrap_or_default();
                *pending = vec![None; displays.len()];
            }
            Ok(Message::Stop) | Err(TryRecvError::Disconnected) => return false,
            Err(TryRecvError::Empty) => return true,
        }
    }
}

fn write(displays: &[Control], pending: &mut [Option<u8>]) {
    for (display, value) in pending.iter_mut().enumerate() {
        let Some(percent) = value.take() else {
            continue;
        };
        let Some(control) = displays.get(display) else {
            continue;
        };

        if let Err(problem) = control.set(Brightness::from_percent(f32::from(percent))) {
            eprintln!("klart-tray: {}: {problem}", control.name());
        }
    }
}
