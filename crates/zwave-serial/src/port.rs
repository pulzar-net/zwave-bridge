//! Opening the link to a controller: a local serial device, or a TCP socket
//! for sticks shared over the network (ser2net, socat, a simulator).

use tokio::io::{AsyncRead, AsyncWrite};

/// A bidirectional byte stream to a controller.
pub trait Port: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Port for T {}

/// The Serial API's fixed line settings: 115200 baud, 8N1, no flow control.
pub const BAUD_RATE: u32 = 115_200;

/// Opens `path`: `tcp://host:port` for a network link, anything else as a
/// local serial device.
pub async fn open(path: &str) -> std::io::Result<Box<dyn Port>> {
    if let Some(addr) = path.strip_prefix("tcp://") {
        let s = tokio::net::TcpStream::connect(addr).await?;
        s.set_nodelay(true)?;
        return Ok(Box::new(s));
    }
    open_serial(path).await
}

/// How long [`open`] waits for a busy serial port to be released.
#[cfg(feature = "serialport")]
const BUSY_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// A port its last user just closed stays locked until the kernel has
/// finished closing it (the tty's close delay, half a second by default), so
/// reopening at once (a restart) finds it busy. `NoDevice` is that busy lock;
/// a missing device is `NotFound`, which fails at once.
#[cfg(feature = "serialport")]
async fn open_serial(path: &str) -> std::io::Result<Box<dyn Port>> {
    use tokio_serial::SerialPortBuilderExt;
    let builder = tokio_serial::new(path, BAUD_RATE)
        .data_bits(tokio_serial::DataBits::Eight)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .flow_control(tokio_serial::FlowControl::None);
    let deadline = tokio::time::Instant::now() + BUSY_WAIT;
    loop {
        match builder.clone().open_native_async() {
            Ok(port) => return Ok(Box::new(port)),
            Err(e)
                if e.kind == tokio_serial::ErrorKind::NoDevice
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => return Err(std::io::Error::other(e)),
        }
    }
}

#[cfg(not(feature = "serialport"))]
async fn open_serial(path: &str) -> std::io::Result<Box<dyn Port>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("{path}: built without serial port support; use tcp://"),
    ))
}
