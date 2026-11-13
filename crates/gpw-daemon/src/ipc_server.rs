//! IPC server: JSON lines (gpw_core::ipc) over a Unix socket (Linux, mode 0666) or a named pipe
//! (Windows, authenticated users). One thread per client; requests that change the guard are
//! handed to the measuring loop through a channel, snapshots are read from shared memory.
use gpw_core::config::Config;
use gpw_core::ipc::{self, Reply, Request, Snapshot};
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Local clients at most (the socket is world writable).
const MAX_CLIENTS: usize = 32;
/// Longest accepted request line.
const MAX_LINE: usize = 64 * 1024;

/// Requests the measuring loop must execute.
pub enum Command {
    Release(String),
    SetConfig(Config),
}

pub type CommandMsg = (Command, Sender<Reply>);

pub struct Shared {
    snapshot: Mutex<Option<(Snapshot, Instant)>>,
    subs: Mutex<Vec<Sender<Arc<str>>>>,
    cmd: Mutex<Sender<CommandMsg>>,
    clients: AtomicUsize,
}

impl Shared {
    pub fn new(cmd: Sender<CommandMsg>) -> Arc<Self> {
        Arc::new(Shared {
            snapshot: Mutex::new(None),
            subs: Mutex::new(Vec::new()),
            cmd: Mutex::new(cmd),
            clients: AtomicUsize::new(0),
        })
    }

    /// Stores the newest snapshot and pushes it to every subscriber.
    pub fn publish(&self, snap: Snapshot) {
        let line: Arc<str> = ipc::encode(&Reply::Snapshot(snap.clone())).into();
        *self.snapshot.lock().unwrap() = Some((snap, Instant::now()));
        self.subs.lock().unwrap().retain(|tx| tx.send(line.clone()).is_ok());
    }

    fn current(&self) -> Reply {
        match &*self.snapshot.lock().unwrap() {
            Some((s, at)) => {
                let mut s = s.clone();
                s.age_ms = at.elapsed().as_millis() as u64;
                Reply::Snapshot(s)
            }
            None => Reply::Error { message: "no reading yet".into() },
        }
    }

    fn command(&self, c: Command) -> Reply {
        let (tx, rx) = mpsc::channel();
        if self.cmd.lock().unwrap().send((c, tx)).is_err() {
            return Reply::Error { message: "service stopping".into() };
        }
        rx.recv_timeout(Duration::from_secs(3)).unwrap_or(Reply::Error { message: "timeout".into() })
    }
}

/// A connected client as the per-client loop needs it.
pub trait Conn {
    /// `Ok(None)` = no data right now (the call waits up to ~100 ms), `Ok(Some(0))` = closed.
    fn read_some(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>>;
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
}

fn handle(shared: &Shared, line: &str, subs: &mut Option<Receiver<Arc<str>>>) -> Reply {
    match ipc::decode::<Request>(line) {
        Err(e) => Reply::Error { message: format!("bad request: {e}") },
        Ok(Request::GetSnapshot) => shared.current(),
        Ok(Request::Subscribe) => {
            if subs.is_none() {
                let (tx, rx) = mpsc::channel();
                shared.subs.lock().unwrap().push(tx);
                *subs = Some(rx);
            }
            shared.current()
        }
        Ok(Request::Release { pci }) => shared.command(Command::Release(pci)),
        Ok(Request::SetConfig { config }) => shared.command(Command::SetConfig(config)),
    }
}

/// Per-client loop: answer each request line, forward pushed snapshots after Subscribe.
fn serve<C: Conn>(mut conn: C, shared: Arc<Shared>) {
    let mut buf = vec![0u8; 4096];
    let mut pending: Vec<u8> = Vec::new();
    let mut subs: Option<Receiver<Arc<str>>> = None;
    'client: loop {
        match conn.read_some(&mut buf) {
            Ok(Some(0)) | Err(_) => break,
            Ok(Some(n)) => {
                pending.extend_from_slice(&buf[..n]);
                while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=pos).collect();
                    let reply = match std::str::from_utf8(&line) {
                        Ok(l) if l.trim().is_empty() => continue,
                        Ok(l) => handle(&shared, l, &mut subs),
                        Err(_) => Reply::Error { message: "bad request: not UTF-8".into() },
                    };
                    if conn.write_all(ipc::encode(&reply).as_bytes()).is_err() {
                        break 'client;
                    }
                }
                if pending.len() > MAX_LINE {
                    break;
                }
            }
            Ok(None) => {}
        }
        if let Some(rx) = &subs {
            while let Ok(line) = rx.try_recv() {
                if conn.write_all(line.as_bytes()).is_err() {
                    break 'client;
                }
            }
        }
    }
    shared.clients.fetch_sub(1, Ordering::SeqCst);
}

fn spawn_client<C: Conn + Send + 'static>(conn: C, shared: &Arc<Shared>) {
    if shared.clients.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
        shared.clients.fetch_sub(1, Ordering::SeqCst);
        return; // dropping closes it
    }
    let shared = shared.clone();
    std::thread::spawn(move || serve(conn, shared));
}

#[cfg(unix)]
mod os {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    impl Conn for UnixStream {
        fn read_some(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
            match self.read(buf) {
                Ok(n) => Ok(Some(n)),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => Ok(None),
                Err(e) => Err(e),
            }
        }
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            Write::write_all(self, data)
        }
    }

    pub fn start(path: &str, shared: Arc<Shared>) -> Result<(), String> {
        let p = Path::new(path);
        if p.exists() {
            if UnixStream::connect(p).is_ok() {
                return Err(format!("{path}: another instance is running"));
            }
            let _ = std::fs::remove_file(p); // stale socket of a killed run
        }
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let listener = UnixListener::bind(p).map_err(|e| format!("{path}: {e}"))?;
        // Every local user may read and ask; every value is sanitised by the service.
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o666)).map_err(|e| format!("{path}: {e}"))?;
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                spawn_client(stream, &shared);
            }
        });
        Ok(())
    }

    pub fn stop(path: &str) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(windows)]
mod os {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PeekNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    /// SYSTEM and administrators: full access. Authenticated users: read + write data, but not
    /// FILE_CREATE_PIPE_INSTANCE (no squatting on the pipe name).
    const SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;AU)";

    struct Pipe(HANDLE);
    // The handle is used by exactly one client thread.
    unsafe impl Send for Pipe {}

    impl Drop for Pipe {
        fn drop(&mut self) {
            unsafe {
                FlushFileBuffers(self.0);
                DisconnectNamedPipe(self.0);
                CloseHandle(self.0);
            }
        }
    }

    impl Conn for Pipe {
        fn read_some(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
            let mut avail = 0u32;
            let ok = unsafe {
                PeekNamedPipe(self.0, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut())
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            if avail == 0 {
                std::thread::sleep(Duration::from_millis(50));
                return Ok(None);
            }
            let want = avail.min(buf.len() as u32);
            let mut read = 0u32;
            if unsafe { ReadFile(self.0, buf.as_mut_ptr(), want, &mut read, std::ptr::null_mut()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Some(read as usize))
        }
        fn write_all(&mut self, mut data: &[u8]) -> io::Result<()> {
            while !data.is_empty() {
                let mut done = 0u32;
                if unsafe { WriteFile(self.0, data.as_ptr(), data.len() as u32, &mut done, std::ptr::null_mut()) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                data = &data[done as usize..];
            }
            Ok(())
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Sd(PSECURITY_DESCRIPTOR);
    unsafe impl Send for Sd {}

    pub fn start(name: &str, shared: Arc<Shared>) -> Result<(), String> {
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let sddl = wide(SDDL);
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } == 0 {
            return Err(format!("security descriptor: {}", io::Error::last_os_error()));
        }
        let sd = Sd(sd); // used for every pipe instance for the whole process life, never freed
        let name = wide(name);
        std::thread::spawn(move || {
            let sd = sd;
            loop {
                let sa = SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: sd.0,
                    bInheritHandle: 0,
                };
                let h = unsafe {
                    CreateNamedPipeW(
                        name.as_ptr(),
                        PIPE_ACCESS_DUPLEX,
                        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                        PIPE_UNLIMITED_INSTANCES,
                        65536,
                        65536,
                        0,
                        &sa,
                    )
                };
                if h == INVALID_HANDLE_VALUE {
                    crate::guard::log(&format!("named pipe: {}", io::Error::last_os_error()));
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                let connected = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0
                    || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                if connected {
                    spawn_client(Pipe(h), &shared);
                } else {
                    unsafe { CloseHandle(h) };
                }
            }
        });
        Ok(())
    }

    pub fn stop(_name: &str) {}
}

pub use os::{start, stop};
