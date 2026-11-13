//! Named pipe transport (a client opens the pipe like a file).
use std::fs::OpenOptions;
use std::io;
use std::time::Duration;

pub const DEFAULT_ENDPOINT: &str = gpw_core::ipc::PIPE_NAME;

/// Pipe handles opened as files have no read timeout; a dead service closes the pipe instead.
pub fn connect(path: &str, _read_timeout: Duration) -> io::Result<super::Conn> {
    let f = OpenOptions::new().read(true).write(true).open(path)?;
    let r = f.try_clone()?;
    Ok((Box::new(r), Box::new(f)))
}
