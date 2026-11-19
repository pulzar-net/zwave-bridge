//! Link layer behavior against a scripted controller on an in-memory pipe.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use zwave_serial::frame::{decode_all, Frame, ACK, CAN, NAK};
use zwave_serial::{Config, DataFrame, FunctionId, Request, SerialApi, SerialError};

fn fast() -> Config {
    Config {
        ack_timeout: Duration::from_millis(100),
        ..Config::default()
    }
}

/// Reads until at least one complete data frame arrived; returns all frames seen.
async fn read_frames(io: &mut DuplexStream) -> Vec<Frame> {
    let mut buf = Vec::new();
    loop {
        let mut chunk = [0u8; 256];
        let n = io.read(&mut chunk).await.unwrap();
        assert!(n > 0, "host closed");
        buf.extend_from_slice(&chunk[..n]);
        let frames = decode_all(&buf);
        if frames.iter().any(|f| matches!(f, Frame::Data(_))) {
            return frames;
        }
    }
}

async fn data(io: &mut DuplexStream) -> DataFrame {
    read_frames(io)
        .await
        .into_iter()
        .find_map(|f| match f {
            Frame::Data(d) => Some(d),
            _ => None,
        })
        .unwrap()
}

#[tokio::test]
async fn request_response() {
    let (host, mut ctrl) = tokio::io::duplex(1024);
    let (api, _u) = SerialApi::start(host, fast());
    let task = tokio::spawn(async move {
        let req = data(&mut ctrl).await;
        assert_eq!(req.function, 0x20);
        ctrl.write_all(&[ACK]).await.unwrap();
        ctrl.write_all(&DataFrame::response(0x20u8, vec![0xc0, 0xff, 0xee, 0x01, 0x01]).encode()).await.unwrap();
        let mut b = [0u8; 1];
        ctrl.read_exact(&mut b).await.unwrap();
        assert_eq!(b[0], ACK, "host acknowledges the response");
    });
    let r = api
        .request(DataFrame::request(FunctionId::GET_CONTROLLER_ID, vec![]), Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(r.payload, vec![0xc0, 0xff, 0xee, 0x01, 0x01]);
    task.await.unwrap();
}

#[tokio::test]
async fn retransmits_after_nak_and_can() {
    let (host, mut ctrl) = tokio::io::duplex(1024);
    let (api, _u) = SerialApi::start(host, fast());
    let task = tokio::spawn(async move {
        data(&mut ctrl).await;
        ctrl.write_all(&[NAK]).await.unwrap();
        data(&mut ctrl).await;
        ctrl.write_all(&[CAN]).await.unwrap();
        data(&mut ctrl).await;
        ctrl.write_all(&[ACK]).await.unwrap();
        ctrl.write_all(&DataFrame::response(0x15u8, b"Z-Wave 7.21\0\x07".to_vec()).encode()).await.unwrap();
        // Keep the pipe open until the host is done.
        let mut b = [0u8; 1];
        let _ = ctrl.read(&mut b).await;
    });
    let r = api
        .request(DataFrame::request(FunctionId::GET_CONTROLLER_VERSION, vec![]), Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(r.function, 0x15);
    drop(api);
    task.abort();
}

#[tokio::test]
async fn gives_up_without_ack() {
    let (host, mut ctrl) = tokio::io::duplex(1024);
    let (api, _u) = SerialApi::start(host, fast());
    let counter = tokio::spawn(async move {
        let mut n = 0;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 64];
        while let Ok(Ok(k)) = tokio::time::timeout(Duration::from_millis(2000), ctrl.read(&mut chunk)).await {
            if k == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..k]);
            n = decode_all(&buf).iter().filter(|f| matches!(f, Frame::Data(_))).count();
            if n == 3 {
                break;
            }
        }
        (n, ctrl)
    });
    let e = api
        .request(DataFrame::request(FunctionId::GET_CONTROLLER_ID, vec![]), Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(e, SerialError::NoAck(3));
    assert_eq!(counter.await.unwrap().0, 3);
}

#[tokio::test]
async fn callback_and_unsolicited() {
    let (host, mut ctrl) = tokio::io::duplex(4096);
    let (api, mut unsolicited) = SerialApi::start(host, fast());
    let cb = api.next_callback_id();
    let task = tokio::spawn(async move {
        let req = data(&mut ctrl).await;
        assert_eq!(req.function, 0x13);
        let cb = *req.payload.last().unwrap();
        ctrl.write_all(&[ACK]).await.unwrap();
        ctrl.write_all(&DataFrame::response(0x13u8, vec![0x01]).encode()).await.unwrap();
        // An unrelated frame from a node arrives before the callback.
        ctrl.write_all(&DataFrame::request(0x04u8, vec![0, 2, 3, 0x20, 0x03, 0xff]).encode()).await.unwrap();
        // A frame with a bad checksum must be NAKed.
        let mut bad = DataFrame::request(0x04u8, vec![0, 2, 2, 0x20, 0x02]).encode();
        let l = bad.len() - 1;
        bad[l] ^= 0x55;
        ctrl.write_all(&bad).await.unwrap();
        ctrl.write_all(&DataFrame::request(0x13u8, vec![cb, 0x00]).encode()).await.unwrap();
        let mut seen = Vec::new();
        let mut chunk = [0u8; 16];
        while seen.iter().filter(|&&b| b == ACK).count() < 3 {
            let n = ctrl.read(&mut chunk).await.unwrap();
            seen.extend_from_slice(&chunk[..n]);
        }
        assert!(seen.contains(&NAK));
    });
    let ex = api
        .execute(Request::with_callback(
            zwave_serial::message::send_data(2, &[0x20, 0x02], 0x25, cb),
            Duration::from_secs(1),
            cb,
            Duration::from_secs(2),
        ))
        .await
        .unwrap();
    assert_eq!(ex.callback.unwrap().payload, vec![cb, 0x00]);
    let u = unsolicited.recv().await.unwrap();
    assert_eq!(u.function, 0x04);
    task.await.unwrap();
}

#[tokio::test]
async fn refused_request_skips_callback() {
    let (host, mut ctrl) = tokio::io::duplex(1024);
    let (api, _u) = SerialApi::start(host, fast());
    tokio::spawn(async move {
        data(&mut ctrl).await;
        ctrl.write_all(&[ACK]).await.unwrap();
        ctrl.write_all(&DataFrame::response(0x13u8, vec![0x00]).encode()).await.unwrap();
        let mut b = [0u8; 8];
        let _ = ctrl.read(&mut b).await;
    });
    let ex = api
        .execute(Request::with_callback(
            zwave_serial::message::send_data(2, &[0x20, 0x02], 0x25, 9),
            Duration::from_secs(1),
            9,
            Duration::from_secs(5),
        ))
        .await
        .unwrap();
    assert_eq!(ex.response.unwrap().payload, vec![0]);
    assert!(ex.callback.is_none());
}

#[tokio::test]
async fn probe_identifies_a_controller_and_ignores_silence() {
    let (host, mut ctrl) = tokio::io::duplex(1024);
    tokio::spawn(async move {
        let req = data(&mut ctrl).await;
        assert_eq!(req.function, 0x07);
        ctrl.write_all(&[ACK]).await.unwrap();
        let caps = zwave_serial::message::SerialApiCapabilities {
            firmware_major: 7,
            firmware_minor: 22,
            manufacturer_id: 0,
            product_type: 4,
            product_id: 4,
            functions: [0xff; 32],
        };
        ctrl.write_all(&DataFrame::response(0x07u8, caps.encode()).encode()).await.unwrap();
        let mut b = [0u8; 8];
        let _ = ctrl.read(&mut b).await;
    });
    let id = zwave_serial::probe::probe_port(host).await.unwrap().unwrap();
    assert_eq!(id.firmware, "7.22");
    assert!(id.known_name().unwrap().contains("800"));

    let (host, _silent) = tokio::io::duplex(1024);
    assert_eq!(zwave_serial::probe::probe_port(host).await.unwrap(), None);
}
