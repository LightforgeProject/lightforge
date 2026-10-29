pub(super) fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        match value {
            0 => {
                out.push(byte);
                return;
            }
            _ => out.push(byte | 0x80),
        }
    }
}

pub(super) fn key(out: &mut Vec<u8>, field: u32, wire: u8) {
    varint(out, u64::from(field) << 3 | u64::from(wire));
}

pub(super) fn varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    key(out, field, 0);
    varint(out, value);
}

pub(super) fn string_field(out: &mut Vec<u8>, field: u32, value: &str) {
    key(out, field, 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

pub(super) fn message_field(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    key(out, field, 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

pub(super) fn double_field(out: &mut Vec<u8>, field: u32, value: f64) {
    key(out, field, 1);
    out.extend_from_slice(&value.to_le_bytes());
}

/// Read one varint, returning its value and how many bytes it used.
pub(super) fn read_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, byte) in bytes.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// One protobuf field's payload, in the two forms this file uses.
pub(super) enum Wire<'a> {
    Bytes(&'a [u8]),
    /// A varint or fixed-width field. Nothing this reader wants is stored as
    /// one, so the value is skipped, not carried.
    Scalar,
}

/// Walks the fields of one protobuf message, stopping at the first byte it
/// cannot make sense of.
pub(super) struct Fields<'a> {
    rest: &'a [u8],
}

impl<'a> Fields<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = self.rest.split_first()?;
            self.rest = rest;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.rest.split_at_checked(n)?;
        self.rest = rest;
        Some(head)
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = (u32, Wire<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.varint()?;
        let field = u32::try_from(key >> 3).ok()?;
        let value = match key & 7 {
            0 => {
                self.varint()?;
                Wire::Scalar
            }
            1 => {
                self.take(8)?;
                Wire::Scalar
            }
            2 => {
                let len = usize::try_from(self.varint()?).ok()?;
                Wire::Bytes(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                Wire::Scalar
            }
            // Groups are long gone from the wire format and Agent emits none.
            _ => return None,
        };
        Some((field, value))
    }
}
