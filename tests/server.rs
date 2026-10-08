// SPDX-License-Identifier: BSD-3-Clause
use imanlib::{
    client::Client,
    igui_iman_stub::Host as ImanStub,
    protocol::{Message, Outcome, Request, Response, REQUEST_LIMIT, RESPONSE_LIMIT},
    server::Server,
    transport::Channel,
};
use serde_json::json;
use std::{
    io::{self, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

fn call(host: &mut ImanStub, client: &mut Client, command: &str) -> Response {
    let id = client.send(command, json!({"extra":true})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(host.poll().is_empty());
        if let Some(response) = client.poll().unwrap() {
            assert_eq!(response.id, id);
            return response;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn fixture() -> (ImanStub, Client) {
    let mut host = ImanStub::bind(Path::new("/dev/shm")).unwrap();
    host.expect_gui(std::process::id()).unwrap();
    let mut gui = Client::connect(host.path()).unwrap();
    assert!(matches!(
        call(&mut host, &mut gui, "igui.attach").outcome,
        Outcome::Done { .. }
    ));
    (host, gui)
}

#[test]
fn non_tmpfs_parent_is_refused_before_endpoint_creation() {
    let error = Server::bind(Path::new("/proc")).err().unwrap();

    assert!(error.to_string().contains("IPC requires tmpfs"));
}

#[test]
fn service_request_limit_counts_ignored_fields_in_the_raw_frame() {
    let mut server = Server::bind(Path::new("/dev/shm")).unwrap();
    let mut sender = UnixStream::connect(server.path()).unwrap();
    let request = json!({"kind":"request","id":1,"command":"igui.bootstrap","params":{},"extra":"x".repeat(REQUEST_LIMIT)});
    let body = serde_json::to_vec(&request).unwrap();
    sender.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
    sender.write_all(&body).unwrap();

    let polled = server.poll(None, Some(std::process::id()));

    assert!(polled.incoming.is_empty());
    assert_eq!(polled.errors.len(), 1);
    assert_eq!(polled.errors[0].operation, "request");
    assert!(polled.errors[0].error.to_string().contains("IPC request limit exceeded"));
}

#[test]
fn multiple_clients_receive_only_their_own_service_outcomes() {
    let (mut host, mut gui) = fixture();
    let path = host.path().to_owned();
    let mut first = Client::connect(&path).unwrap();
    let mut second = Client::connect(&path).unwrap();
    let first_id = first.send("igui.input", json!({"action":"down"})).unwrap();
    let second_id = second.send("igui.screenshot", json!({})).unwrap();
    assert_eq!(first_id, second_id); // independent ID domains, rewritten by the router
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut commands = Vec::new();
    while commands.len() < 2 {
        assert!(host.poll().is_empty());
        assert!(first.poll().unwrap().is_none());
        assert!(second.poll().unwrap().is_none());
        assert!(gui.poll().unwrap().is_none());
        if let Some(request) = gui.take_request() {
            commands.push(request);
        }
        assert!(Instant::now() < deadline);
    }
    assert_ne!(commands[0].id, commands[1].id);
    // Receiving/forwarding alone cannot report success. Actual completion is required.
    for request in commands.iter().rev() {
        gui.reply(Response::done(
            request.id,
            json!({"processed":request.command}),
        ))
        .unwrap();
    }
    let (mut a, mut b) = (None, None);
    while a.is_none() || b.is_none() {
        assert!(host.poll().is_empty());
        gui.poll().unwrap();
        if a.is_none() {
            a = first.poll().unwrap();
        }
        if b.is_none() {
            b = second.poll().unwrap();
        }
        assert!(Instant::now() < deadline);
    }
    assert!(
        matches!(a.unwrap().outcome, Outcome::Done { result } if result["processed"] == "igui.input")
    );
    assert!(
        matches!(b.unwrap().outcome, Outcome::Done { result } if result["processed"] == "igui.screenshot")
    );
    drop(host);
    assert!(!path.exists());
}
#[test]
fn only_expected_pid_and_registered_connection_can_attach_and_close() {
    let mut host = ImanStub::bind(Path::new("/dev/shm")).unwrap();
    host.expect_gui(u32::MAX).unwrap();
    let mut wrong = Client::connect(host.path()).unwrap();
    assert!(
        matches!(call(&mut host, &mut wrong, "igui.attach").outcome, Outcome::Error { error } if error.code == "forbidden")
    );
    assert!(host.expect_gui(std::process::id()).is_err());
    let (mut host, _gui) = fixture();
    let mut wrong = Client::connect(host.path()).unwrap();
    for command in ["igui.attach", "igui.closed"] {
        assert!(
            matches!(call(&mut host, &mut wrong, command).outcome, Outcome::Error { error } if error.code == "forbidden")
        );
    }
    assert!(!host.closed());
}
#[test]
fn service_disconnect_does_not_turn_pending_action_into_success() {
    let (mut host, mut gui) = fixture();
    let mut client = Client::connect(host.path()).unwrap();
    client.send("igui.input", json!({"action":"b"})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(host.poll().is_empty());
        client.poll().unwrap();
        gui.poll().unwrap();
        if gui.take_request().is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    drop(gui);
    loop {
        assert!(host.poll().is_empty());
        if let Some(response) = client.poll().unwrap() {
            assert!(
                matches!(response.outcome, Outcome::Error { error } if error.code == "delivery_unknown")
            );
            break;
        }
        assert!(Instant::now() < deadline);
    }
}
#[test]
fn stalled_service_times_out_without_retry_or_false_acknowledgement() {
    let (mut host, mut gui) = fixture();
    let mut client = Client::connect(host.path()).unwrap();
    client.send("igui.input", json!({"action":"b"})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut deliveries = 0;
    loop {
        assert!(host.poll().is_empty());
        gui.poll().unwrap();
        if gui.take_request().is_some() {
            deliveries += 1;
        }
        if let Some(response) = client.poll().unwrap() {
            assert!(
                matches!(response.outcome, Outcome::Error { error } if error.code == "delivery_unknown")
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(deliveries, 1);
}

#[test]
fn malformed_peer_is_reported_without_leaking_payload_or_discarding_healthy_requests() {
    let mut server = Server::bind(Path::new("/dev/shm")).unwrap();
    let mut invalid = UnixStream::connect(server.path()).unwrap();
    let body = br#"{"kind":"request","id":"private-value","command":"core.status","params":{}}"#;
    invalid.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
    invalid.write_all(body).unwrap();
    let mut healthy = Client::connect(server.path()).unwrap();
    let id = healthy.send("core.status", json!({})).unwrap();
    assert!(healthy.poll().unwrap().is_none());

    let polled = server.poll(None, None);

    assert_eq!(polled.errors.len(), 1);
    let failure = &polled.errors[0];
    assert_eq!(failure.operation, "read");
    assert_eq!(failure.error.kind(), io::ErrorKind::InvalidData);
    assert!(failure.to_string().contains("Invalid IPC JSON message"));
    assert!(!failure.to_string().contains("private-value"));
    assert!(!server.connected(failure.peer.unwrap()));
    assert_eq!(polled.incoming.len(), 1);
    let request = &polled.incoming[0];
    assert_eq!(request.request.id, id);
    assert!(server.connected(request.peer));
    server.send(request.peer, &Message::Response(Response::done(id, json!({})))).unwrap();
    assert!(server.poll(None, None).errors.is_empty());
    assert!(matches!(healthy.poll().unwrap().unwrap().outcome, Outcome::Done { .. }));
}

#[test]
fn clean_disconnect_is_normal_but_truncated_frames_are_reported() {
    let mut server = Server::bind(Path::new("/dev/shm")).unwrap();
    drop(UnixStream::connect(server.path()).unwrap());

    let clean = server.poll(None, None);

    assert!(clean.errors.is_empty());
    assert!(clean.incoming.is_empty());
    let mut truncated = UnixStream::connect(server.path()).unwrap();
    truncated.write_all(&[0, 0]).unwrap();
    drop(truncated);

    let partial = server.poll(None, None);

    assert!(partial.incoming.is_empty());
    assert_eq!(partial.errors.len(), 1);
    assert_eq!(partial.errors[0].error.kind(), io::ErrorKind::UnexpectedEof);
    assert!(!server.connected(partial.errors[0].peer.unwrap()));
}

#[test]
fn refused_connections_are_reported_without_removing_existing_peers() {
    let mut server = Server::bind(Path::new("/dev/shm")).unwrap();
    let mut connected = Vec::new();
    for _ in 0..8 {
        connected.push(UnixStream::connect(server.path()).unwrap());
    }
    assert!(server.poll(None, Some(std::process::id())).errors.is_empty());
    let _extra = UnixStream::connect(server.path()).unwrap();

    let polled = server.poll(None, Some(std::process::id()));

    assert_eq!(polled.errors.len(), 1);
    assert!(polled.errors[0].peer.is_none());
    assert_eq!(polled.errors[0].error.kind(), io::ErrorKind::ConnectionRefused);
    assert!(polled.errors[0].to_string().contains("connection limit"));
    for peer in 1..=8 {
        assert!(server.connected(peer));
    }
}

#[test]
fn failed_forwarded_reply_is_reported_and_leaves_no_partial_frame_or_replay() {
    let mut server = Server::bind(Path::new("/dev/shm")).unwrap();
    let mut service = Client::connect(server.path()).unwrap();
    service.send("igui.attach", json!({})).unwrap();
    service.poll().unwrap();
    let attached = server.poll(None, Some(std::process::id()));
    assert!(attached.errors.is_empty());
    let service_peer = attached.incoming[0].peer;
    let mut origin = Channel::new(UnixStream::connect(server.path()).unwrap(), RESPONSE_LIMIT).unwrap();
    origin.queue(&Message::Request(Request::new(u64::MAX, "igui.screenshot", json!({}))), REQUEST_LIMIT).unwrap();
    origin.flush().unwrap();
    let polled = server.poll(Some(service_peer), Some(std::process::id()));
    assert!(polled.errors.is_empty());
    let incoming = &polled.incoming[0];
    let origin_peer = incoming.peer;
    server.forward(service_peer, incoming).unwrap();
    assert!(server.poll(Some(service_peer), Some(std::process::id())).errors.is_empty());
    service.poll().unwrap();
    let forwarded = service.take_request().unwrap();
    let empty = Response::done(forwarded.id, json!(""));
    let overhead = serde_json::to_vec(&Message::Response(empty)).unwrap().len();
    let full = Response::done(forwarded.id, json!("x".repeat(RESPONSE_LIMIT - overhead)));
    service.reply(full).unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let failures = loop {
        service.poll().unwrap();
        let polled = server.poll(Some(service_peer), Some(std::process::id()));
        if !polled.errors.is_empty() { break polled.errors; }
        assert!(Instant::now() < deadline);
    };

    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].peer, Some(origin_peer));
    assert_eq!(failures[0].operation, "reply");
    assert!(failures[0].to_string().contains("output limit"));
    assert!(origin.receive::<Message>().unwrap().is_none());
    assert!(service.take_request().is_none());
    server.send(origin_peer, &Message::Response(Response::error(
        u64::MAX, "delivery_unknown", "Synthetic owner response; no retry",
    ))).unwrap();
    assert!(server.poll(Some(service_peer), Some(std::process::id())).errors.is_empty());
    assert!(matches!(origin.receive::<Message>().unwrap(), Some(Message::Response(response)) if response.id == u64::MAX));
}

#[test]
fn stub_capabilities_ignore_extra_parameters_and_do_not_advertise_a_protocol_version() {
    let (mut host, mut gui) = fixture();

    let response = call(&mut host, &mut gui, "core.capabilities");

    let Outcome::Done { result } = response.outcome else { panic!("Expected capabilities"); };
    assert!(!result.as_object().unwrap().contains_key("version"));
    assert_eq!(result["mode"], "gui_only");
    assert_eq!(result["commands"], json!(["igui.status", "igui.input", "igui.screenshot", "igui.close"]));
}
