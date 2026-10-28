//! Pumps bytes between a local API socket and async code. Local sockets are
//! blocking (`interprocess`), so one thread per stream polls the socket, as
//! the phone gateway's API proxy does. Stopping writes what is already
//! queued first, so an error line queued just before closing arrives.

use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::Duration;

use tokio::sync::mpsc;

use crate::ipc::{LocalStream, LocalStreamReadCount};

const READ_CHUNK: usize = 64 * 1024;
const POLL_MIN: Duration = Duration::from_millis(2);
const POLL_MAX: Duration = Duration::from_millis(50);

/// The writing end of a piped local socket. Dropping it closes the socket
/// once queued bytes are written.
pub(crate) struct PipeWriter {
    outbound: std_mpsc::Sender<Vec<u8>>,
    stop: Arc<AtomicBool>,
}

impl PipeWriter {
    /// Queues bytes for the socket; `false` once the socket is gone.
    pub(crate) fn write(&self, bytes: Vec<u8>) -> bool {
        self.outbound.send(bytes).is_ok()
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Starts pumping `stream`. The receiver yields what the socket sends and
/// ends when it closes.
pub(crate) fn start(mut stream: LocalStream) -> io::Result<(PipeWriter, mpsc::Receiver<Vec<u8>>)> {
    crate::ipc::set_local_stream_polling(&mut stream, true)?;
    let (inbound_tx, inbound) = mpsc::channel(64);
    let (outbound, outbound_rx) = std_mpsc::channel::<Vec<u8>>();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::Builder::new()
        .name("flock-pipe".into())
        .spawn(move || pump(stream, &inbound_tx, &outbound_rx, &stopped))?;
    Ok((PipeWriter { outbound, stop }, inbound))
}

fn pump(
    mut stream: LocalStream,
    inbound: &mpsc::Sender<Vec<u8>>,
    outbound: &std_mpsc::Receiver<Vec<u8>>,
    stop: &AtomicBool,
) {
    let mut buffer = vec![0_u8; READ_CHUNK];
    let mut idle = POLL_MIN;
    loop {
        let stopping = stop.load(Ordering::Acquire);
        let mut busy = false;
        loop {
            match outbound.try_recv() {
                Ok(bytes) => {
                    if write_all_polling(&mut stream, &bytes).is_err() {
                        return;
                    }
                    busy = true;
                }
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => return,
            }
        }
        if stopping {
            return;
        }
        match crate::ipc::poll_local_stream_read_count(&mut stream, &mut buffer) {
            Ok(LocalStreamReadCount::Data(count)) => {
                if inbound.blocking_send(buffer[..count].to_vec()).is_err() {
                    return;
                }
                busy = true;
            }
            Ok(LocalStreamReadCount::Pending) => {}
            Ok(LocalStreamReadCount::Closed) | Err(_) => return,
        }
        if busy {
            idle = POLL_MIN;
        } else {
            std::thread::sleep(idle);
            idle = (idle * 2).min(POLL_MAX);
        }
    }
}

fn write_all_polling(stream: &mut LocalStream, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => bytes = &bytes[written..],
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(POLL_MIN),
            Err(err) => return Err(err),
        }
    }
    stream.flush()
}

/// Splits a byte stream into lines.
#[derive(Default)]
pub(crate) struct LineBuffer {
    partial: Vec<u8>,
}

impl LineBuffer {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.partial.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(end) = self.partial.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            lines.push(line);
        }
        lines
    }
}
