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
    open_serial(path)
}

#[cfg(feature = "serialport")]
fn open_serial(path: &str) -> std::io::Result<Box<dyn Port>> {
    use tokio_serial::SerialPortBuilderExt;
    let port = tokio_serial::new(path, BAUD_RATE)
        .data_bits(tokio_serial::DataBits::Eight)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .flow_control(tokio_serial::FlowControl::None)
        .open_native_async()
        .map_err(std::io::Error::other)?;
    Ok(Box::new(port))
}

#[cfg(not(feature = "serialport"))]
fn open_serial(path: &str) -> std::io::Result<Box<dyn Port>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("{path}: built without serial port support; use tcp://"),
    ))
}
