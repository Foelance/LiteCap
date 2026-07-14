use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

/// Bind an ephemeral TCP loopback listener; returns it plus the bound port
/// so the caller can pass `tcp://127.0.0.1:<port>` to ffmpeg as an input.
pub fn listener() -> Result<(TcpListener, u16)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

/// Accept the single connection ffmpeg makes back to us, with a timeout.
/// Never blocks forever: ffmpeg failing to connect is a startup error.
pub fn accept_with_timeout(listener: &TcpListener, timeout: Duration) -> Result<TcpStream> {
    listener.set_nonblocking(true)?;
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => {
                listener.set_nonblocking(false)?;
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() >= timeout {
                    bail!("timed out waiting for ffmpeg to connect to audio socket");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.into()),
        }
    }
}
