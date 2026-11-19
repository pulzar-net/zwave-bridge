//! A bounds-checked cursor over a command payload.

use crate::{ParseError, ParseErrorKind, Value};

pub(crate) struct Reader<'a> {
    p: &'a [u8],
    at: usize,
    cc: u16,
    command: u8,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(p: &'a [u8], cc: u16, command: u8) -> Self {
        Reader {
            p,
            at: 0,
            cc,
            command,
        }
    }

    fn err(&self, kind: ParseErrorKind) -> ParseError {
        ParseError {
            cc: self.cc,
            command: self.command,
            kind,
        }
    }

    pub(crate) fn has(&self) -> bool {
        self.at < self.p.len()
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.p.len())
            .ok_or(self.err(ParseErrorKind::TooShort))?;
        let s = &self.p[self.at..end];
        self.at = end;
        Ok(s)
    }

    pub(crate) fn rest(&mut self) -> &'a [u8] {
        let s = &self.p[self.at..];
        self.at = self.p.len();
        s
    }

    pub(crate) fn u8(&mut self) -> Result<u8, ParseError> {
        Ok(self.take(1)?[0])
    }
    #[allow(dead_code)]
    pub(crate) fn i8(&mut self) -> Result<i8, ParseError> {
        Ok(self.u8()? as i8)
    }
    pub(crate) fn u16(&mut self) -> Result<u16, ParseError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    pub(crate) fn i16(&mut self) -> Result<i16, ParseError> {
        Ok(self.u16()? as i16)
    }
    pub(crate) fn u24(&mut self) -> Result<u32, ParseError> {
        let b = self.take(3)?;
        Ok(u32::from_be_bytes([0, b[0], b[1], b[2]]))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, ParseError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(crate) fn i32(&mut self) -> Result<i32, ParseError> {
        Ok(self.u32()? as i32)
    }

    /// A signed big-endian integer of 0, 1, 2 or 4 bytes.
    pub(crate) fn sized(&mut self, size: u8) -> Result<i32, ParseError> {
        Ok(match size {
            0 => 0,
            1 => i32::from(self.u8()? as i8),
            2 => i32::from(self.i16()?),
            4 => self.i32()?,
            s => return Err(self.err(ParseErrorKind::InvalidSize(s))),
        })
    }

    /// A precision/scale/size byte followed by the value.
    pub(crate) fn value(&mut self) -> Result<Value, ParseError> {
        let b = self.u8()?;
        let size = b & 0x07;
        if !matches!(size, 1 | 2 | 4) {
            return Err(self.err(ParseErrorKind::InvalidSize(size)));
        }
        Ok(Value {
            precision: b >> 5,
            scale: (b >> 3) & 0x03,
            size,
            raw: self.sized(size)?,
        })
    }
}
