//! logging.dll - fire-and-forget log file writer for DataFlex.
//!
//! `LogText` never blocks on the file: it puts the message on a queue and
//! returns immediately. A single background thread owns the queue and
//! appends each file's messages once it can open that file exclusively
//! (i.e. nobody else has it open). If the file is in use, the messages stay
//! queued and the write is retried. `LogStatus` reports how many messages
//! are still waiting to be written.

use std::ffi::CStr;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::raw::{c_char, c_long};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

/// How long to wait before retrying a file that could not be opened.
const RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// Messages accepted by LogText that have not yet been written to disk.
static QUEUED: AtomicI32 = AtomicI32::new(0);

static SENDER: OnceLock<Sender<Entry>> = OnceLock::new();

struct Entry {
    filename: String,
    data: Vec<u8>,
}

/// Everything waiting to be written to one file, in arrival order.
struct PendingFile {
    key: String,
    filename: String,
    data: Vec<u8>,
    count: i32,
    retry_at: Option<Instant>,
}

fn sender() -> &'static Sender<Entry> {
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("logging-writer".into())
            .spawn(move || writer_loop(rx))
            .expect("failed to start logging writer thread");
        tx
    })
}

/// Opens the file for appending, refusing to share it with anyone else, so
/// the open fails while another process (or handle) has the file open.
fn open_exclusive(filename: &str) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options.open(filename)
}

fn try_write(pending: &PendingFile) -> io::Result<()> {
    let mut file = open_exclusive(&pending.filename)?;
    file.write_all(&pending.data)?;
    file.flush()
}

fn writer_loop(rx: Receiver<Entry>) {
    let mut pending: Vec<PendingFile> = Vec::new();

    loop {
        // Sleep until a new message arrives or the earliest retry is due.
        let next_retry = pending.iter().filter_map(|p| p.retry_at).min();
        let received = match next_retry {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
        };
        match received {
            Ok(entry) => add_entry(&mut pending, entry),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Batch up anything else already queued so each file is opened once.
        while let Ok(entry) = rx.try_recv() {
            add_entry(&mut pending, entry);
        }

        let now = Instant::now();
        pending.retain_mut(|p| {
            if p.retry_at.is_some_and(|at| at > now) {
                return true;
            }
            match try_write(p) {
                Ok(()) => {
                    QUEUED.fetch_sub(p.count, Ordering::SeqCst);
                    false
                }
                Err(_) => {
                    p.retry_at = Some(now + RETRY_INTERVAL);
                    true
                }
            }
        });
    }
}

fn add_entry(pending: &mut Vec<PendingFile>, entry: Entry) {
    // Windows paths are case-insensitive, so "App.log" and "app.log" share a queue.
    let key = entry.filename.to_ascii_lowercase();
    match pending.iter_mut().find(|p| p.key == key) {
        Some(p) => {
            p.data.extend_from_slice(&entry.data);
            p.count += 1;
        }
        None => pending.push(PendingFile {
            key,
            filename: entry.filename,
            data: entry.data,
            count: 1,
            retry_at: None,
        }),
    }
}

/// Queues `ps_message` to be appended (followed by CRLF) to `ps_filename`.
/// Returns 0 when queued, -1 if either pointer is null or the filename is
/// not valid text, -2 if the filename does not end in ".log".
#[no_mangle]
pub extern "system" fn LogText(ps_filename: *const c_char, ps_message: *const c_char) -> c_long {
    if ps_filename.is_null() || ps_message.is_null() {
        return -1;
    }

    let filename = match unsafe { CStr::from_ptr(ps_filename) }.to_str() {
        Ok(s) => s.trim(),
        Err(_) => return -1,
    };
    if !filename.to_ascii_lowercase().ends_with(".log") {
        return -2;
    }

    // Write the message bytes as given, whatever code page the caller uses.
    let mut data = unsafe { CStr::from_ptr(ps_message) }.to_bytes().to_vec();
    if !data.ends_with(b"\n") {
        data.extend_from_slice(b"\r\n");
    }

    QUEUED.fetch_add(1, Ordering::SeqCst);
    if sender()
        .send(Entry { filename: filename.to_string(), data })
        .is_err()
    {
        QUEUED.fetch_sub(1, Ordering::SeqCst);
        return -1;
    }

    0
}

/// Returns the number of messages queued by LogText that have not yet been
/// written to their log files.
#[no_mangle]
pub extern "system" fn LogStatus() -> c_long {
    QUEUED.load(Ordering::SeqCst) as c_long
}

/// Waits up to `timeout_ms` milliseconds for the queue to empty. Call this
/// before the application exits: anything still queued when the process ends
/// is lost. Returns the number of messages still queued (0 = all written).
#[no_mangle]
pub extern "system" fn LogFlush(timeout_ms: c_long) -> c_long {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0) as u64);
    while QUEUED.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    LogStatus()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn log(filename: &str, message: &str) -> c_long {
        let f = CString::new(filename).unwrap();
        let m = CString::new(message).unwrap();
        LogText(f.as_ptr(), m.as_ptr())
    }

    fn temp_log(name: &str) -> String {
        let path = std::env::temp_dir().join(format!("logging-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn rejects_non_log_filenames() {
        assert_eq!(log("c:\\temp\\file.txt", "x"), -2);
        assert_eq!(LogText(std::ptr::null(), std::ptr::null()), -1);
    }

    #[test]
    fn appends_messages_in_order() {
        let path = temp_log("order.log");
        for i in 0..100 {
            assert_eq!(log(&path, &format!("line {i}")), 0);
        }
        assert_eq!(LogFlush(5000), 0);

        let text = std::fs::read_to_string(&path).unwrap();
        let expected: String = (0..100).map(|i| format!("line {i}\r\n")).collect();
        assert_eq!(text, expected);
        std::fs::remove_file(&path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn waits_while_file_is_open_elsewhere() {
        let path = temp_log("locked.log");
        std::fs::write(&path, "first\r\n").unwrap();

        let holder = File::open(&path).unwrap();
        assert_eq!(log(&path, "second"), 0);
        thread::sleep(Duration::from_millis(600));
        assert!(LogStatus() >= 1, "message should stay queued while file is open");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\r\n");

        drop(holder);
        assert_eq!(LogFlush(5000), 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\r\nsecond\r\n");
        std::fs::remove_file(&path).unwrap();
    }
}
