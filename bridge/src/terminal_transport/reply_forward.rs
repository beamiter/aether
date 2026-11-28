use super::{decode_record, invalid, read_record_bytes, Reply, VERSION};
use serde::{
    de::{MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{
    fmt,
    io::{self, BufRead, Write},
};

pub(super) fn forward_reply(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    request: u32,
    limit: usize,
) -> io::Result<()> {
    let bytes = read_record_bytes(reader, limit)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "terminal owner closed during request",
        )
    })?;
    let reply: Reply<ValidatedJson> = decode_record(&bytes)?;
    if reply.protocol != VERSION || reply.request != request {
        return Err(invalid("terminal owner reply does not match request"));
    }
    writer.write_all(&bytes)?;
    writer.flush()
}

// Use serde_json's normal parsing at every level without allocating a tree.
// In particular, strings and nesting use the same checks as Value.
struct ValidatedJson;

impl<'de> Deserialize<'de> for ValidatedJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Self)
    }
}

impl<'de> Visitor<'de> for ValidatedJson {
    type Value = Self;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_str<E>(self, _: &str) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut elements: A) -> Result<Self, A::Error> {
        while elements.next_element::<Self>()?.is_some() {}
        Ok(self)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<Self, A::Error> {
        while fields.next_entry::<Self, Self>()?.is_some() {}
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn forwards_the_validated_record_without_reencoding_it() {
        let record = concat!(
            r#"{"result": {"Ok": {"glyph\u0031":"雪", "cells":[null,true,4,-1,2.5,"\ud83d\ude42"]}}, "request":7, "protocol":1}"#,
            " \r\n"
        );
        let mut bytes = Vec::new();
        forward_reply(&mut record.as_bytes(), &mut bytes, 7, record.len()).unwrap();
        assert_eq!(bytes, record.as_bytes());
    }

    #[test]
    fn rejects_corrupt_packets_before_forwarding_bytes() {
        let invalid_records: &[&[u8]] = &[
            b"",
            b"{\"protocol\":1,\"request\":7,\"result\":{\"Ok\":null}}",
            b"{\"protocol\":1,\"request\":7,\"result\":{\"Ok\":[null]}} trailing\n",
            b"{\"protocol\":2,\"request\":7,\"result\":{\"Ok\":null}}\n",
            b"{\"protocol\":1,\"request\":8,\"result\":{\"Ok\":null}}\n",
            b"{\"protocol\":1,\"request\":7,\"result\":{\"Ok\":null},\"extra\":true}\n",
            b"{\"protocol\":1,\"request\":7,\"result\":{\"Err\":42}}\n",
            b"{\"protocol\":1,\"request\":7,\"result\":{\"Ok\":{\"cells\":[1,]}}}\n",
            b"{\"protocol\":1,\"request\":7,\"result\":{\"Ok\":{\"cells\":[\"\\ud800\"]}}}\n",
            b"{\"protocol\":1,\"request\":7,\"result\":{\"Ok\":{\"cells\":[\"\xff\"]}}}\n",
        ];
        for mut record in invalid_records.iter().copied() {
            let mut output = Vec::new();
            assert!(forward_reply(&mut record, &mut output, 7, 1024).is_err());
            assert!(output.is_empty());
        }
        let oversized = b"{\"protocol\":1,\"request\":7,\"result\":{\"Ok\":null}}\n";
        let mut output = Vec::new();
        assert!(forward_reply(
            &mut oversized.as_slice(),
            &mut output,
            7,
            oversized.len() - 1
        )
        .is_err());
        assert!(output.is_empty());

        let deep = format!(
            "{{\"protocol\":1,\"request\":7,\"result\":{{\"Ok\":{}0{}}}}}\n",
            "[".repeat(256),
            "]".repeat(256)
        );
        assert!(decode_record::<Reply<Value>>(deep.as_bytes()).is_err());
        assert!(forward_reply(&mut deep.as_bytes(), &mut output, 7, 1024).is_err());
        assert!(output.is_empty());
    }
}
