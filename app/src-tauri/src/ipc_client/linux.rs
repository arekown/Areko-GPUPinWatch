//! Unix socket transport.
use std::io;
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub const DEFAULT_ENDPOINT: &str = gpw_core::ipc::SOCKET_PATH;

pub fn connect(path: &str, read_timeout: Duration) -> io::Result<super::Conn> {
    let s = UnixStream::connect(path)?;
    s.set_read_timeout(Some(read_timeout))?;
    s.set_write_timeout(Some(Duration::from_secs(2)))?;
    let r = s.try_clone()?;
    Ok((Box::new(r), Box::new(s)))
}
