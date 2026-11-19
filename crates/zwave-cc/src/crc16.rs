//! CRC-16 Encapsulation and the CRC Z-Wave uses (CRC-CCITT, polynomial
//! 0x1021, initial value 0x1D0F; the `crc` crate's `CRC_16_SPI_FUJITSU`).

/// The CRC-16 Encapsulation command class id.
pub const CC: u8 = 0x56;

const ALG: crc::Crc<u16> = crc::Crc::<u16>::new(&crc::CRC_16_SPI_FUJITSU);

/// The Z-Wave CRC-16 of `data`.
pub fn checksum(data: &[u8]) -> u16 {
    ALG.checksum(data)
}

/// Wraps a command in CRC-16 Encapsulation.
pub fn encapsulate(command: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(command.len() + 4);
    v.extend_from_slice(&[CC, 0x01]);
    v.extend_from_slice(command);
    let c = checksum(&v);
    v.extend_from_slice(&c.to_be_bytes());
    v
}

/// Unwraps a CRC-16 Encapsulation frame (starting at `0x56 0x01`); `None`
/// when it is too short or the CRC does not match.
pub fn decapsulate(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < 5 || frame[0] != CC || frame[1] != 0x01 {
        return None;
    }
    let (body, crc) = frame.split_at(frame.len() - 2);
    (checksum(body) == u16::from_be_bytes([crc[0], crc[1]])).then_some(&body[2..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_value_and_roundtrip() {
        // The algorithm's published check value for "123456789".
        assert_eq!(checksum(b"123456789"), 0xe5cc);
        let f = encapsulate(&[0x20, 0x02]);
        assert_eq!(decapsulate(&f), Some(&[0x20u8, 0x02][..]));
        let mut bad = f.clone();
        bad[2] ^= 1;
        assert_eq!(decapsulate(&bad), None);
    }
}
