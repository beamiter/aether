use super::{decode_record, invalid, read_record, read_record_bytes, write_record, Reply, VERSION};
use crate::SSH_DENSE_FRAME_VERSION;
use serde::{
    de::{DeserializeOwned, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::{Map, Value};
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
    let bytes = read_record_bytes(reader, limit)?.ok_or_else(owner_closed)?;
    let reply: Reply<ValidatedJson> = decode_record(&bytes)?;
    check_reply(&reply, request)?;
    writer.write_all(&bytes)?;
    writer.flush()
}

pub(super) fn forward_status_reply(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    request: u32,
    limit: usize,
) -> io::Result<()> {
    forward_metadata_reply(reader, writer, request, limit, |status| {
        // A current owner and an older stdio helper can share the same socket.
        // Advertise the command through a relay that can also decode it.
        status.insert("packed_frame_relay".into(), Value::Bool(true));
        status.insert(
            "packed_frame_relay_version".into(),
            SSH_DENSE_FRAME_VERSION.into(),
        );
    })
}

pub(super) fn forward_identify_reply(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    request: u32,
    limit: usize,
) -> io::Result<()> {
    forward_metadata_reply(reader, writer, request, limit, |identity| {
        identity.insert(
            "packed_snapshot_relay_version".into(),
            SSH_DENSE_FRAME_VERSION.into(),
        );
    })
}

fn forward_metadata_reply(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    request: u32,
    limit: usize,
    annotate: impl FnOnce(&mut Map<String, Value>),
) -> io::Result<()> {
    let mut reply: Reply = read_record(reader, limit)?.ok_or_else(owner_closed)?;
    check_reply(&reply, request)?;
    if let Ok(Value::Object(metadata)) = &mut reply.result {
        annotate(metadata);
    }
    write_record(writer, &reply, limit)
}

fn owner_closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "terminal owner closed during request",
    )
}

fn check_reply<T>(reply: &Reply<T>, request: u32) -> io::Result<()> {
    if reply.protocol != VERSION || reply.request != request {
        return Err(invalid("terminal owner reply does not match request"));
    }
    Ok(())
}

/// Apply serde_json's string and nesting checks to the complete reply
/// before retaining a raw JSON frame.
pub(super) fn read_validated_reply<T: DeserializeOwned>(
    reader: &mut impl BufRead,
    limit: usize,
) -> io::Result<Option<Reply<T>>> {
    let Some(bytes) = read_record_bytes(reader, limit)? else {
        return Ok(None);
    };
    let _: Reply<ValidatedJson> = decode_record(&bytes)?;
    decode_record(&bytes).map(Some)
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
    use crate::terminal_transport::RawPollReply;
    use serde_json::Value;

    #[test]
    fn identify_capabilities_accept_legacy_ids_and_mark_current_relays() {
        let identity = "a".repeat(32);
        for result in [
            Value::String(identity.clone()),
            serde_json::json!({"instance":identity,"packed_snapshot_version":SSH_DENSE_FRAME_VERSION}),
        ] {
            let is_legacy = result.is_string();
            let probe = super::super::IDENTIFY_CAPABILITIES_REQUEST;
            let mut owner_reply = Vec::new();
            write_record(
                &mut owner_reply,
                &Reply {
                    protocol: VERSION,
                    request: probe,
                    result: Ok(result),
                },
                1024,
            )
            .unwrap();
            let mut response = Vec::new();
            forward_identify_reply(&mut owner_reply.as_slice(), &mut response, probe, 1024)
                .unwrap();
            let received: Reply = decode_record(&response).unwrap();
            let value = received.result.unwrap();
            if is_legacy {
                assert_eq!(value, identity);
            } else {
                assert_eq!(value["instance"], identity);
                assert_eq!(
                    value["packed_snapshot_relay_version"],
                    SSH_DENSE_FRAME_VERSION
                );
            }
        }
    }

    #[test]
    fn status_reply_advertises_the_stdio_helper_capability() {
        let packet =
            b"{\"protocol\":1,\"request\":7,\"result\":{\"Ok\":{\"packed_frame_version\":8}}}\n";
        let mut forwarded = Vec::new();
        forward_status_reply(&mut packet.as_slice(), &mut forwarded, 7, 1024).unwrap();
        let reply: Reply = decode_record(&forwarded).unwrap();
        let status = reply.result.unwrap();
        assert_eq!(status["packed_frame_version"], 8);
        assert_eq!(status["packed_frame_relay"], true);
        assert_eq!(
            status["packed_frame_relay_version"],
            SSH_DENSE_FRAME_VERSION
        );
    }

    #[test]
    fn client_keeps_the_frame_fragment_after_validating_the_reply() {
        let frame = r#"{ "kind":"snapshot", "frame":{"text":"雪\u0021","revision":8} }"#;
        let packet = format!(
            "{{\"protocol\":1,\"request\":7,\"result\":{{\"Ok\":{{\"update\":{frame},\"status\":{{\"epoch\":3}}}}}}}}\n"
        );
        let receipt: Reply<RawPollReply> =
            read_validated_reply(&mut packet.as_bytes(), packet.len())
                .unwrap()
                .unwrap();
        assert_eq!((receipt.protocol, receipt.request), (VERSION, 7));
        let reply = receipt.result.unwrap();
        assert_eq!(reply.update.get(), frame);
        assert_eq!(reply.status["epoch"], 3);

        let corrupt = packet.replace(r"雪\u0021", r"\ud800");
        assert!(
            read_validated_reply::<RawPollReply>(&mut corrupt.as_bytes(), packet.len()).is_err()
        );
        let missing_update =
            b"{\"protocol\":1,\"request\":7,\"result\":{\"Ok\":{\"status\":{\"epoch\":3}}}}\n";
        assert!(
            read_validated_reply::<RawPollReply>(&mut missing_update.as_slice(), packet.len())
                .is_err()
        );
    }

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
