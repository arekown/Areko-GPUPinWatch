//! IPC client of the service (contract: gpw_core::ipc, JSON lines).
//! One long-lived subscription connection receives the pushed snapshots; every command
//! (release, settings) opens its own short connection and waits for the single reply.
//! Endpoint: environment variable `GPW_SOCKET`, otherwise the platform default.
use gpw_core::ipc::{self, Reply, Request, Snapshot};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::time::Duration;

#[cfg(unix)]
mod linux;
#[cfg(unix)]
use linux as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;

/// Pause between connection attempts while the service is not reachable.
const RETRY: Duration = Duration::from_secs(2);
/// No snapshot for this long (service pushes at least 1 Hz) = connection is dead.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(10);
/// The service answers commands within 3 s.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

type Conn = (Box<dyn Read + Send>, Box<dyn Write + Send>);

pub fn endpoint() -> String {
    std::env::var("GPW_SOCKET").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| os::DEFAULT_ENDPOINT.to_string())
}

/// Sends one request on a fresh connection; `Err` carries the service's message or the I/O error.
pub fn request(req: &Request) -> Result<(), String> {
    let path = endpoint();
    let (r, mut w) = os::connect(&path, COMMAND_TIMEOUT).map_err(|e| format!("{path}: {e}"))?;
    w.write_all(ipc::encode(req).as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(r).read_line(&mut line).map_err(|e| e.to_string())?;
    match ipc::decode::<Reply>(&line).map_err(|e| e.to_string())? {
        Reply::Ok | Reply::Snapshot(_) => Ok(()),
        Reply::Error { message } => Err(message),
    }
}

/// Runs forever: subscribes, hands every snapshot to `on` (`None` = connection lost or refused),
/// reconnects every [`RETRY`].
pub fn subscribe(mut on: impl FnMut(Option<Snapshot>, &str)) -> ! {
    loop {
        let path = endpoint();
        let why = match os::connect(&path, SUBSCRIBE_TIMEOUT) {
            Ok(conn) => read_snapshots(conn, |s| on(Some(s), "")),
            Err(e) => e,
        };
        on(None, &format!("{path}: {why}"));
        std::thread::sleep(RETRY);
    }
}

fn read_snapshots((r, mut w): Conn, mut on: impl FnMut(Snapshot)) -> io::Error {
    if let Err(e) = w.write_all(ipc::encode(&Request::Subscribe).as_bytes()) {
        return e;
    }
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return io::Error::new(io::ErrorKind::UnexpectedEof, "service closed the connection"),
            Ok(_) => match ipc::decode::<Reply>(&line) {
                Ok(Reply::Snapshot(s)) => on(s),
                // "no reading yet" right after Subscribe; pushes follow
                Ok(_) => {}
                Err(e) => return io::Error::new(io::ErrorKind::InvalidData, e),
            },
            Err(e) => return e,
        }
    }
}
