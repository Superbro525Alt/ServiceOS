//! Bounds-checked wire helpers over fixed buffers (RFC 4253 §4-§6 shapes):
//! byte/u32 access, `string` and `mpint` encoding, and in-place name-list
//! iteration. Everything returns subslices — no allocation.

/// Wire-level failures. They all map to honest disconnects upstream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireErr {
    /// Input ended before the declared field completed.
    Truncated,
    /// Output buffer too small for the value being written.
    Overflow,
}

/// Reader over a byte slice with SSH primitive accessors.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn u8(&mut self) -> Result<u8, WireErr> {
        if self.pos >= self.buf.len() {
            return Err(WireErr::Truncated);
        }
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn u32(&mut self) -> Result<u32, WireErr> {
        if self.remaining() < 4 {
            return Err(WireErr::Truncated);
        }
        let b = &self.buf[self.pos..self.pos + 4];
        self.pos += 4;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Take exactly `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], WireErr> {
        if self.remaining() < n {
            return Err(WireErr::Truncated);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// SSH `string`: u32 length prefix + bytes.
    pub fn string(&mut self) -> Result<&'a [u8], WireErr> {
        let len = self.u32()? as usize;
        self.take(len)
    }
}

/// Writer into a fixed buffer with SSH primitive emission.
pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Writer<'a> {
        Writer { buf, pos: 0 }
    }

    pub fn len(&self) -> usize {
        self.pos
    }

    pub fn into_written(self) -> usize {
        self.pos
    }

    fn room(&self, n: usize) -> Result<(), WireErr> {
        if self.pos + n > self.buf.len() {
            Err(WireErr::Overflow)
        } else {
            Ok(())
        }
    }

    pub fn u8(&mut self, v: u8) -> Result<(), WireErr> {
        self.room(1)?;
        self.buf[self.pos] = v;
        self.pos += 1;
        Ok(())
    }

    pub fn u32(&mut self, v: u32) -> Result<(), WireErr> {
        self.room(4)?;
        self.buf[self.pos..self.pos + 4].copy_from_slice(&v.to_be_bytes());
        self.pos += 4;
        Ok(())
    }

    /// Raw bytes, no prefix.
    pub fn raw(&mut self, bytes: &[u8]) -> Result<(), WireErr> {
        self.room(bytes.len())?;
        self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
        self.pos += bytes.len();
        Ok(())
    }

    /// SSH `string`: u32 length prefix + bytes.
    pub fn string(&mut self, bytes: &[u8]) -> Result<(), WireErr> {
        self.u32(bytes.len() as u32)?;
        self.raw(bytes)
    }

    /// SSH `mpint` for a non-negative big-endian magnitude: leading zero
    /// bytes are stripped, one 0x00 is prepended when the top bit of the
    /// first remaining byte is set, and zero encodes as the empty string
    /// (RFC 4253 §5).
    pub fn mpint_be(&mut self, magnitude: &[u8]) -> Result<(), WireErr> {
        let mut start = 0;
        while start < magnitude.len() && magnitude[start] == 0 {
            start += 1;
        }
        let body = &magnitude[start..];
        if body.is_empty() {
            return self.u32(0);
        }
        let lead = if body[0] & 0x80 != 0 { 1 } else { 0 };
        self.u32((body.len() + lead) as u32)?;
        if lead == 1 {
            self.u8(0)?;
        }
        self.raw(body)
    }
}

/// In-place iteration over a comma-separated name-list body (the bytes
/// inside a `string`, not including the length prefix). An empty body is an
/// empty list; empty segments between commas are skipped.
pub struct NameList<'a> {
    rest: &'a [u8],
}

impl<'a> NameList<'a> {
    pub fn new(body: &'a [u8]) -> NameList<'a> {
        NameList { rest: body }
    }

    pub fn is_empty(body: &[u8]) -> bool {
        body.is_empty()
    }
}

impl<'a> Iterator for NameList<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        loop {
            if self.rest.is_empty() {
                return None;
            }
            let end = self
                .rest
                .iter()
                .position(|&b| b == b',')
                .unwrap_or(self.rest.len());
            let (name, tail) = self.rest.split_at(end);
            self.rest = if end < self.rest.len() {
                &tail[1..]
            } else {
                &tail[..]
            };
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_string_bounds() {
        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(&12u32.to_be_bytes());
        buf[4..16].copy_from_slice(&b"helloworld12"[..12]);
        let mut r = Reader::new(&buf);
        assert_eq!(r.string().unwrap(), &b"helloworld12"[..]);
        assert_eq!(r.string().unwrap_err(), WireErr::Truncated);
    }

    #[test]
    fn reader_take_truncated() {
        let mut r = Reader::new(&[1u8, 2, 3]);
        assert_eq!(r.take(4).unwrap_err(), WireErr::Truncated);
        assert_eq!(r.take(3).unwrap(), &[1, 2, 3]);
    }

    #[test]
    fn writer_overflow() {
        let mut buf = [0u8; 3];
        let mut w = Writer::new(&mut buf);
        assert_eq!(w.u32(7).unwrap_err(), WireErr::Overflow);
        let mut buf2 = [0u8; 7];
        let mut w2 = Writer::new(&mut buf2);
        w2.u32(1).unwrap();
        assert_eq!(w2.raw(&[1, 2, 3, 4]).unwrap_err(), WireErr::Overflow);
    }

    #[test]
    fn mpint_encoding_rfc4253_s5() {
        // RFC 4253 §5 vectors: zero -> empty, leading zero stripped,
        // high bit padded, multi-byte passthrough.
        let cases: &[(&[u8], &[u8])] = &[
            (&[0x00, 0x00, 0x00, 0x00], &[0, 0, 0, 0]),
            (&[0x00, 0x00, 0x00, 0x80], &[0, 0, 0, 2, 0x00, 0x80]),
            (&[0x00, 0x00, 0x00, 0x7f], &[0, 0, 0, 1, 0x7f]),
            (
                &[0x9c, 0x41, 0x56, 0x23],
                &[0, 0, 0, 5, 0x00, 0x9c, 0x41, 0x56, 0x23],
            ),
            (
                &[0x00, 0x9c, 0x41, 0x56],
                &[0, 0, 0, 4, 0x00, 0x9c, 0x41, 0x56],
            ),
        ];
        for (input, expect) in cases {
            let mut buf = [0u8; 32];
            let mut w = Writer::new(&mut buf);
            w.mpint_be(input).unwrap();
            let n = w.into_written();
            assert_eq!(&buf[..n], *expect, "input {:?}", input);
        }
    }

    #[test]
    fn mpint_zero_is_empty_string() {
        let mut buf = [0u8; 8];
        let mut w = Writer::new(&mut buf);
        w.mpint_be(&[0, 0, 0]).unwrap();
        assert_eq!(w.into_written(), 4);
        assert_eq!(&buf[..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn namelist_iteration() {
        let body = b"curve25519-sha256,curve25519-sha256@libssh.org";
        let names: Vec<&[u8]> = NameList::new(body).collect();
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], &b"curve25519-sha256"[..]);
        assert_eq!(names[1], &b"curve25519-sha256@libssh.org"[..]);

        // Empty list and empty segments.
        assert_eq!(NameList::new(b"").count(), 0);
        assert_eq!(NameList::new(b",,x,").count(), 1);
    }

    #[test]
    fn writer_string_roundtrip() {
        let mut buf = [0u8; 16];
        let mut w = Writer::new(&mut buf);
        w.string(b"abc").unwrap();
        let n = w.into_written();
        let mut r = Reader::new(&buf[..n]);
        assert_eq!(r.string().unwrap(), &b"abc"[..]);
    }
}
