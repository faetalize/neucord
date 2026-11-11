//! The part of protobuf's wire format Discord's user settings need, read and
//! written without a code generator or a schema: Neucord names the field
//! numbers it cares about, and every other field passes through as it came.

/// One field's payload, by wire type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

/// A message's fields in the order they were sent. A field sent twice keeps
/// both; the accessors read the last, as protobuf does for single values.
#[derive(Clone, Debug, Default)]
pub struct Message<'a> {
    fields: Vec<(u32, Wire<'a>)>,
}

fn varint(buf: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for (i, byte) in buf.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            *buf = &buf[i + 1..];
            return Some(value);
        }
    }
    None
}

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn push_tag(out: &mut Vec<u8>, number: u32, wire_type: u64) {
    push_varint(out, u64::from(number) << 3 | wire_type);
}

fn push_field(out: &mut Vec<u8>, number: u32, wire: Wire) {
    match wire {
        Wire::Varint(v) => {
            push_tag(out, number, 0);
            push_varint(out, v);
        }
        Wire::Fixed64(v) => {
            push_tag(out, number, 1);
            out.extend(v.to_le_bytes());
        }
        Wire::Bytes(bytes) => {
            push_tag(out, number, 2);
            push_varint(out, bytes.len() as u64);
            out.extend(bytes);
        }
        Wire::Fixed32(v) => {
            push_tag(out, number, 5);
            out.extend(v.to_le_bytes());
        }
    }
}

/// A value to write into a message: a varint (booleans, enums, integers) or
/// a length-delimited payload (strings, nested messages, wrappers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Varint(u64),
    Bytes(Vec<u8>),
}

/// A `BoolValue`, which is empty for `false`: proto3 leaves out a default.
pub fn bool_value(value: bool) -> Value {
    Value::Bytes(if value { vec![0x08, 1] } else { Vec::new() })
}

/// A `UInt32Value`, empty for 0.
pub fn uint_value(value: u32) -> Value {
    Value::Bytes(match value {
        0 => Vec::new(),
        v => {
            let mut inner = vec![0x08];
            push_varint(&mut inner, u64::from(v));
            inner
        }
    })
}

fn take<'a>(buf: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    let (head, rest) = buf.split_at_checked(len)?;
    *buf = rest;
    Some(head)
}

fn push_value(out: &mut Vec<u8>, number: u32, value: &Value) {
    match value {
        Value::Varint(v) => push_field(out, number, Wire::Varint(*v)),
        Value::Bytes(bytes) => push_field(out, number, Wire::Bytes(bytes)),
    }
}

impl<'a> Message<'a> {
    /// `None` if `buf` is not a well-formed message (cut short, or using
    /// the group wire types nothing here sends).
    pub fn parse(mut buf: &'a [u8]) -> Option<Self> {
        let mut fields = Vec::new();
        while !buf.is_empty() {
            let tag = varint(&mut buf)?;
            let number = u32::try_from(tag >> 3).ok().filter(|n| *n > 0)?;
            let wire = match tag & 7 {
                0 => Wire::Varint(varint(&mut buf)?),
                1 => Wire::Fixed64(u64::from_le_bytes(take(&mut buf, 8)?.try_into().ok()?)),
                2 => {
                    let len = usize::try_from(varint(&mut buf)?).ok()?;
                    Wire::Bytes(take(&mut buf, len)?)
                }
                5 => Wire::Fixed32(u32::from_le_bytes(take(&mut buf, 4)?.try_into().ok()?)),
                _ => return None,
            };
            fields.push((number, wire));
        }
        Some(Message { fields })
    }

    fn last(&self, number: u32) -> Option<Wire<'a>> {
        self.fields.iter().rev().find(|(n, _)| *n == number).map(|(_, w)| *w)
    }

    /// A length-delimited field's payload (a nested message, a string);
    /// `None` if the field is absent.
    pub fn bytes(&self, number: u32) -> Option<&'a [u8]> {
        match self.last(number)? {
            Wire::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// The top-level fields holding a nested message, by number: what a
    /// partial update replaces.
    pub fn nested(&self) -> Vec<(u32, &'a [u8])> {
        self.fields
            .iter()
            .filter_map(|(n, wire)| match wire {
                Wire::Bytes(bytes) => Some((*n, *bytes)),
                _ => None,
            })
            .collect()
    }

    /// The message with `number` set to `value` (or removed, for `None`) and
    /// every other field as it was, unknown ones included. The new field
    /// goes where its number falls among the others, which is where a
    /// protobuf library writes it.
    pub fn with(&self, number: u32, value: Option<Value>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut value = value;
        for (n, wire) in self.fields.iter().filter(|(n, _)| *n != number) {
            if *n > number
                && let Some(value) = value.take()
            {
                push_value(&mut out, number, &value);
            }
            push_field(&mut out, *n, *wire);
        }
        if let Some(value) = value {
            push_value(&mut out, number, &value);
        }
        out
    }

    /// A nested message; `None` if the field is absent or malformed.
    pub fn message(&self, number: u32) -> Option<Message<'a>> {
        match self.last(number)? {
            Wire::Bytes(bytes) => Message::parse(bytes),
            _ => None,
        }
    }

    pub fn varint(&self, number: u32) -> Option<u64> {
        match self.last(number)? {
            Wire::Varint(v) => Some(v),
            _ => None,
        }
    }

    pub fn bool(&self, number: u32) -> Option<bool> {
        self.varint(number).map(|v| v != 0)
    }

    pub fn string(&self, number: u32) -> Option<&'a str> {
        match self.last(number)? {
            Wire::Bytes(bytes) => std::str::from_utf8(bytes).ok(),
            _ => None,
        }
    }

    /// A repeated fixed64, whether sent packed or one by one.
    pub fn fixed64s(&self, number: u32) -> Vec<u64> {
        let mut values = Vec::new();
        for (n, wire) in &self.fields {
            match (*n == number, wire) {
                (true, Wire::Fixed64(v)) => values.push(*v),
                (true, Wire::Bytes(bytes)) => {
                    values.extend(bytes.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)))
                }
                _ => {}
            }
        }
        values
    }

    /// The repeated messages of a field.
    pub fn messages(&self, number: u32) -> Vec<Message<'a>> {
        self.fields
            .iter()
            .filter_map(|(n, wire)| match (*n == number, wire) {
                (true, Wire::Bytes(bytes)) => Message::parse(bytes),
                _ => None,
            })
            .collect()
    }

    // Google's wrapper messages (`BoolValue` and friends) give a scalar a
    // presence: an absent field is "unset", while a wrapper holding the
    // default value is sent as an empty message.

    pub fn bool_value(&self, number: u32) -> Option<bool> {
        Some(self.message(number)?.bool(1).unwrap_or(false))
    }

    pub fn string_value(&self, number: u32) -> Option<&'a str> {
        Some(self.message(number)?.string(1).unwrap_or(""))
    }

    /// An `Int64Value`, `UInt64Value`, `Int32Value` or `UInt32Value`.
    pub fn int_value(&self, number: u32) -> Option<u64> {
        Some(self.message(number)?.varint(1).unwrap_or(0))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Encoders for the tests, which build messages the way Discord's
    /// server does.
    pub fn varint_bytes(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    pub fn varint_field(number: u32, v: u64) -> Vec<u8> {
        [varint_bytes(u64::from(number) << 3), varint_bytes(v)].concat()
    }

    pub fn bytes_field(number: u32, payload: &[u8]) -> Vec<u8> {
        [varint_bytes(u64::from(number) << 3 | 2), varint_bytes(payload.len() as u64), payload.to_vec()].concat()
    }

    pub fn bool_wrapper(number: u32, v: bool) -> Vec<u8> {
        bytes_field(number, &if v { varint_field(1, 1) } else { Vec::new() })
    }

    #[test]
    fn reads_every_wire_type() {
        let buf = [
            varint_field(1, 300),
            bytes_field(2, b"hi"),
            [varint_bytes(3 << 3 | 1), 7u64.to_le_bytes().to_vec()].concat(),
            [varint_bytes(4 << 3 | 5), 9u32.to_le_bytes().to_vec()].concat(),
        ]
        .concat();
        let message = Message::parse(&buf).unwrap();
        assert_eq!(message.varint(1), Some(300));
        assert_eq!(message.string(2), Some("hi"));
        assert_eq!(message.fixed64s(3), vec![7]);
        assert_eq!(message.last(4), Some(Wire::Fixed32(9)));
        assert_eq!(message.varint(2), None, "wrong wire type");
        assert_eq!(message.varint(9), None, "absent");
    }

    #[test]
    fn wrappers_tell_unset_from_default() {
        let buf = [bool_wrapper(1, false), bool_wrapper(2, true)].concat();
        let message = Message::parse(&buf).unwrap();
        assert_eq!(message.bool_value(1), Some(false), "an empty wrapper is false, not unset");
        assert_eq!(message.bool_value(2), Some(true));
        assert_eq!(message.bool_value(3), None);
    }

    #[test]
    fn the_last_of_a_repeated_single_wins() {
        let buf = [varint_field(1, 1), varint_field(1, 0)].concat();
        assert_eq!(Message::parse(&buf).unwrap().bool(1), Some(false));
    }

    #[test]
    fn fixed64s_packed_or_not() {
        let packed = bytes_field(1, &[1u64.to_le_bytes(), 2u64.to_le_bytes()].concat());
        let loose = [varint_bytes(1 << 3 | 1), 3u64.to_le_bytes().to_vec()].concat();
        let buf = [packed, loose].concat();
        assert_eq!(Message::parse(&buf).unwrap().fixed64s(1), vec![1, 2, 3]);
    }

    #[test]
    fn setting_a_field_keeps_the_others_in_order() {
        let fixed = [varint_bytes(9 << 3 | 1), 7u64.to_le_bytes().to_vec()].concat();
        let buf = [varint_field(1, 1), bytes_field(5, b"x"), fixed.clone(), varint_field(99, 4)].concat();
        let message = Message::parse(&buf).unwrap();

        let set = message.with(3, Some(Value::Varint(300)));
        assert_eq!(
            set,
            [varint_field(1, 1), varint_field(3, 300), bytes_field(5, b"x"), fixed.clone(), varint_field(99, 4)]
                .concat()
        );
        let replaced = message.with(5, Some(bool_value(true)));
        assert_eq!(
            replaced,
            [varint_field(1, 1), bytes_field(5, &[0x08, 1]), fixed.clone(), varint_field(99, 4)].concat()
        );
        let last = message.with(200, Some(Value::Varint(1)));
        assert_eq!(last, [buf.clone(), varint_field(200, 1)].concat(), "after everything, unknown fields too");
        assert_eq!(message.with(1, None), [bytes_field(5, b"x"), fixed, varint_field(99, 4)].concat());
        assert_eq!(Message::parse(&[]).unwrap().with(2, Some(Value::Bytes(vec![]))), bytes_field(2, &[]));
    }

    #[test]
    fn wrappers_are_written_the_way_they_are_read() {
        assert_eq!(bool_value(false), Value::Bytes(vec![]));
        assert_eq!(uint_value(0), Value::Bytes(vec![]));
        let buf = [
            Message::parse(&[]).unwrap().with(1, Some(bool_value(true))),
            Message::parse(&[]).unwrap().with(2, Some(uint_value(300))),
            Message::parse(&[]).unwrap().with(3, Some(bool_value(false))),
        ]
        .concat();
        let message = Message::parse(&buf).unwrap();
        assert_eq!(message.bool_value(1), Some(true));
        assert_eq!(message.int_value(2), Some(300));
        assert_eq!(message.bool_value(3), Some(false), "present, though empty");
        assert_eq!(message.nested().len(), 3);
    }

    #[test]
    fn rejects_malformed() {
        assert!(Message::parse(&[0x08]).is_none(), "tag without a value");
        assert!(Message::parse(&[0x08, 0x80]).is_none(), "varint cut short");
        assert!(Message::parse(&[0x12, 0x05, 0x01]).is_none(), "length past the end");
        assert!(Message::parse(&[0x0b]).is_none(), "start group");
        assert!(Message::parse(&[0x00, 0x01]).is_none(), "field number 0");
        assert!(Message::parse(&[]).unwrap().fields.is_empty());
    }
}
