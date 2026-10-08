// SPDX-License-Identifier: BSD-3-Clause
use imanlib::{
    client::Client,
    protocol::{Message, Outcome, Request, Response, REQUEST_LIMIT, RESPONSE_LIMIT},
    transport::Channel,
};
use serde_json::json;
use std::{
    cell::Cell,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
};

struct StreamingValues<'a> {
    visited: &'a Cell<usize>,
    count: usize,
}
impl serde::Serialize for StreamingValues<'_> {
    fn serialize<Serializer: serde::Serializer>(&self, serializer: Serializer) -> Result<Serializer::Ok, Serializer::Error> {
        use serde::ser::SerializeSeq;

        let mut sequence = serializer.serialize_seq(Some(self.count))?;
        for _ in 0..self.count {
            self.visited.set(self.visited.get() + 1);
            sequence.serialize_element("payload")?;
        }
        sequence.end()
    }
}

struct FailingValue;
impl serde::Serialize for FailingValue {
    fn serialize<Serializer: serde::Serializer>(&self, serializer: Serializer) -> Result<Serializer::Ok, Serializer::Error> {
        use serde::ser::{Error, SerializeSeq};

        let mut sequence = serializer.serialize_seq(None)?;
        sequence.serialize_element("partial")?;
        Err(Serializer::Error::custom("Synthetic serialization failure"))
    }
}

fn frame(value: &impl serde::Serialize) -> Vec<u8> {
    let body = serde_json::to_vec(value).unwrap();
    let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
    bytes.extend(body);
    bytes
}
#[test]
fn fragmented_header_and_payload_and_adjacent_frames_roundtrip() {
    let (mut sender, receiver) = UnixStream::pair().unwrap();
    let mut channel = Channel::new(receiver, REQUEST_LIMIT).unwrap();
    let bytes = frame(&Message::Request(Request::new(7, "core.status", json!({}))));
    for byte in &bytes[..bytes.len() - 1] {
        sender.write_all(&[*byte]).unwrap();
        assert!(channel.receive::<Message>().unwrap().is_none());
    }
    sender.write_all(&bytes[bytes.len() - 1..]).unwrap();
    assert!(matches!(
        channel.receive().unwrap(),
        Some(Message::Request(Request { id: 7, .. }))
    ));
    sender.write_all(&[bytes.clone(), bytes].concat()).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            channel.receive::<Message>().unwrap(),
            Some(Message::Request(_))
        ));
    }
}
#[test]
fn size_headers_and_truncation_are_rejected_before_parsing_or_allocation() {
    for size in [0, REQUEST_LIMIT as u32 + 1, u32::MAX] {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(receiver, REQUEST_LIMIT).unwrap();
        sender.write_all(&size.to_be_bytes()).unwrap();
        assert!(channel
            .receive::<Message>()
            .unwrap_err()
            .to_string()
            .contains("limit"));
    }
    let (mut sender, receiver) = UnixStream::pair().unwrap();
    let mut channel = Channel::new(receiver, REQUEST_LIMIT).unwrap();
    sender.write_all(&[0, 0]).unwrap();
    drop(sender);
    assert!(channel.receive::<Message>().is_err());
}
#[test]
fn response_variants_and_close_event_have_one_wire_format() {
    for value in [
        Message::Response(Response::done(1, json!({"value":3}))),
        Message::Response(Response::error(2, "forbidden", "Not allowed")),
        Message::Response(Response {
            id: 3,
            outcome: Outcome::Accepted { operation_id: 9 },
        }),
        Message::Event {
            event: "igui.close".into(),
            params: json!({}),
        },
    ] {
        let (sender, mut receiver) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(sender, RESPONSE_LIMIT).unwrap();
        channel.queue(&value, RESPONSE_LIMIT).unwrap();
        channel.flush().unwrap();
        let mut header = [0; 4];
        receiver.read_exact(&mut header).unwrap();
        let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
        receiver.read_exact(&mut bytes).unwrap();
        let decoded: Message = serde_json::from_slice(&bytes).unwrap();
        assert!(!serde_json::to_value(&decoded).unwrap().as_object().unwrap().contains_key("version"));
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(value).unwrap()
        );
    }
}
#[test]
fn output_queue_and_command_shape_are_bounded() {
    let (sender, _receiver) = UnixStream::pair().unwrap();
    let mut channel = Channel::new(sender, REQUEST_LIMIT).unwrap();
    for _ in 0..4 {
        channel.queue(&json!({}), REQUEST_LIMIT).unwrap();
    }
    assert!(channel.queue(&json!({}), REQUEST_LIMIT).is_err());
    assert!(channel
        .queue(&"x".repeat(RESPONSE_LIMIT), REQUEST_LIMIT)
        .is_err());
    for name in [
        "status",
        ".status",
        "core.",
        "core.status.extra",
        "core.Status",
        "../exec",
    ] {
        assert!(!Request::new(1, name, json!({})).valid());
    }
    assert!(!Request::new(0, "core.status", json!({})).valid());
    assert!(!Request::new(1, "core.status", json!([])).valid());
    assert!(Request::new(1, "io_monitor.report", json!({})).valid());
}
#[test]
fn malformed_json_missing_required_fields_and_invalid_types_are_rejected() {
    for body in [
        b"{bad".as_slice(),
        br#"{"kind":"request","command":"core.status","params":{}}"#.as_slice(),
        br#"{"kind":"request","id":"one","command":"core.status","params":{}}"#.as_slice(),
        br#"{"kind":"request","id":1,"command":false,"params":{}}"#.as_slice(),
        br#"{"kind":"unexpected","id":1}"#.as_slice(),
        br#"{"kind":"response","id":1,"status":"unexpected","result":{}}"#.as_slice(),
        br#"{"kind":"response","id":1,"status":"accepted","extra":true}"#.as_slice(),
        br#"{"kind":"response","id":1,"status":"accepted","operation_id":"one","extra":true}"#.as_slice(),
        br#"{"kind":"response","id":1,"status":"error","error":{"code":"refused","message":1,"extra":true}}"#.as_slice(),
    ] {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(receiver, REQUEST_LIMIT).unwrap();
        sender
            .write_all(&(body.len() as u32).to_be_bytes())
            .unwrap();
        sender.write_all(body).unwrap();
        assert!(channel.receive::<Message>().is_err());
    }
}

#[test]
fn unversioned_messages_ignore_unknown_fields_without_weakening_known_fields() {
    for value in [
        json!({"kind":"request","id":1,"command":"core.status","params":{"extra":true},"extra":{"nested":1}}),
        json!({"kind":"response","id":2,"status":"done","result":{},"extra":true}),
        json!({"kind":"response","id":3,"status":"accepted","operation_id":4,"extra":true}),
        json!({"kind":"response","id":4,"status":"error","error":{"code":"refused","message":"Synthetic refusal","extra":true},"extra":true}),
        json!({"kind":"event","event":"igui.close","params":{"extra":true},"extra":true}),
    ] {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(receiver, RESPONSE_LIMIT).unwrap();
        sender.write_all(&frame(&value)).unwrap();

        let decoded = channel.receive::<Message>().unwrap().unwrap();
        let normalized = serde_json::to_value(decoded).unwrap();

        assert!(!normalized.as_object().unwrap().contains_key("version"));
        assert!(!normalized.as_object().unwrap().contains_key("extra"));
    }
}

#[test]
fn client_request_limit_counts_ignored_fields_in_the_raw_frame() {
    let directory = tempfile::tempdir_in("/dev/shm").unwrap();
    let socket = directory.path().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut client = Client::connect(&socket).unwrap();
    let (mut sender, _) = listener.accept().unwrap();
    let request = json!({"kind":"request","id":1,"command":"igui.status","params":{},"extra":"x".repeat(REQUEST_LIMIT)});

    sender.write_all(&frame(&request)).unwrap();

    assert!(client.poll().unwrap_err().to_string().contains("unexpected IPC message"));
    assert!(client.take_request().is_none());
}

#[test]
fn output_limit_stops_serialization_and_failed_frames_do_not_corrupt_the_queue() {
    let (sender, receiver) = UnixStream::pair().unwrap();
    let mut writer = Channel::new(sender, REQUEST_LIMIT).unwrap();
    let mut reader = Channel::new(receiver, REQUEST_LIMIT).unwrap();
    writer.queue(&Message::Request(Request::new(1, "core.status", json!({}))), REQUEST_LIMIT).unwrap();
    let visited = Cell::new(0);
    let large = StreamingValues { visited: &visited, count: 1_000_000 };

    assert!(writer.queue(&large, REQUEST_LIMIT).unwrap_err().to_string().contains("IPC output limit exceeded"));
    assert!(visited.get() > 0 && visited.get() <= REQUEST_LIMIT);
    assert!(writer.queue(&FailingValue, REQUEST_LIMIT).unwrap_err().to_string().contains("Synthetic serialization failure"));
    writer.queue(&Message::Request(Request::new(2, "core.status", json!({}))), REQUEST_LIMIT).unwrap();

    for expected_id in [1, 2] {
        writer.flush().unwrap();
        assert!(matches!(reader.receive::<Message>().unwrap(), Some(Message::Request(request)) if request.id == expected_id));
    }
    assert!(!writer.pending_output());
    assert!(reader.receive::<Message>().unwrap().is_none());
}

#[test]
fn full_output_queue_rejects_before_calling_the_serializer() {
    let (sender, _receiver) = UnixStream::pair().unwrap();
    let mut channel = Channel::new(sender, REQUEST_LIMIT).unwrap();
    for _ in 0..4 {
        channel.queue(&json!({}), REQUEST_LIMIT).unwrap();
    }
    let visited = Cell::new(0);
    let value = StreamingValues { visited: &visited, count: 1 };

    assert!(channel.queue(&value, REQUEST_LIMIT).is_err());
    assert_eq!(visited.get(), 0);
}

#[test]
fn exact_output_limit_preserves_json_bytes_and_rejected_frames_leave_no_output() {
    let value = Message::Response(Response::done(1, json!({"text":"Привет\n\"\\"})));
    let expected = frame(&value);
    let body_size = expected.len() - 4;
    let (sender, mut receiver) = UnixStream::pair().unwrap();
    let mut channel = Channel::new(sender, RESPONSE_LIMIT).unwrap();

    for limit in [0, body_size - 1] {
        assert!(channel.queue(&value, limit).is_err());
        assert!(!channel.pending_output());
    }
    channel.queue(&value, body_size).unwrap();
    channel.flush().unwrap();

    let mut received = vec![0; expected.len()];
    receiver.read_exact(&mut received).unwrap();
    assert_eq!(received, expected);
}
