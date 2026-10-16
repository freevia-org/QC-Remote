use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};
fn update(url: String, timeout: i32) -> Vec<u8> {
    pa::UpdaterForwardMessage {
        updater_request_id: 91,
        request: Some(pa::updater_forward_message::Request::Request(
            pa::UpdaterForwardRequest {
                r#type: pa::updater_forward_request_type::Enum::DownloadUpdate as i32,
                url,
                timeout,
                ..Default::default()
            },
        )),
        ..Default::default()
    }
    .encode_to_vec()
}
#[test]
fn shared_entrypoint_streams_error_binary_with_correlated_final_status() {
    let bytes = (0..100_000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let (url, received, server) = http::wire_tests::serve(503, bytes.clone());
    let mut replies = Vec::new();
    forward(
        61,
        &update(url.to_string(), 5000),
        Arc::new(AtomicBool::new(false)),
        &mut |frame, deadline| {
            assert!(deadline.is_some());
            replies.push(frame);
            Ok(())
        },
    )
    .unwrap();
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    server.join().unwrap().unwrap();
    assert_eq!(replies.len(), 4);
    let mut all = Vec::new();
    for (index, frame) in replies.iter().enumerate() {
        assert_eq!(frame.message_type, 61);
        let message = pa::UpdaterForwardMessage::decode(frame.payload.as_slice()).unwrap();
        assert_eq!(message.updater_request_id, 91);
        let Some(pa::updater_forward_message::Response::Response(response)) = message.response
        else {
            panic!("response missing")
        };
        let last = index == 3;
        assert_eq!(response.payload.len(), if last { 1696 } else { 32768 });
        assert_eq!(
            response.is_last_chunk,
            Some(pa::updater_forward_response::IsLastChunk::IsLastChunk(last))
        );
        assert_eq!(
            response.error_code,
            last.then_some(pa::updater_forward_response::ErrorCode::ErrorCode(503))
        );
        all.extend(response.payload);
    }
    assert_eq!(all, bytes);
}
#[test]
fn cancellation_closes_an_active_request_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let trigger = Arc::clone(&cancel);
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut byte = [0];
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(socket.read(&mut byte).unwrap(), 1);
            head.push(byte[0]);
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
            .unwrap();
        trigger.store(true, Ordering::Release);
        socket.read(&mut byte)
    });
    let started = Instant::now();
    let result = forward(
        61,
        &update(format!("http://{address}/"), 0),
        cancel,
        &mut |_, _| Ok(()),
    );
    assert_eq!(result, Err(ForwardError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(1));
    let closed = server.join().unwrap();
    assert!(
        matches!(closed, Ok(0))
            || matches!(closed,Err(ref e) if e.kind()==std::io::ErrorKind::ConnectionReset)
    );
}
#[test]
fn body_progress_does_not_extend_the_absolute_device_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let _ = socket.read(&mut request);
        let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n");
        for _ in 0..20 {
            if socket.write_all(b"x").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let started = Instant::now();
    let result = forward(
        61,
        &update(format!("http://{address}/"), 100),
        Arc::new(AtomicBool::new(false)),
        &mut |_, _| Ok(()),
    );
    assert_eq!(result, Err(ForwardError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(1));
    server.join().unwrap();
}
#[test]
fn notification_and_pre_cancelled_requests_do_not_emit_traffic() {
    let mut called = false;
    forward(
        18,
        b"notification",
        Arc::new(AtomicBool::new(false)),
        &mut |_, _| {
            called = true;
            Ok(())
        },
    )
    .unwrap();
    assert!(!called);
    let result = forward(
        61,
        b"malformed",
        Arc::new(AtomicBool::new(true)),
        &mut |_, _| {
            called = true;
            Ok(())
        },
    );
    assert_eq!(result, Err(ForwardError::Cancelled));
    assert!(!called);
}

#[test]
fn admission_deadline_includes_worker_queue_delay() {
    let mut emitted = false;
    let result = forward_at(
        61,
        &update("http://127.0.0.1:1/must-not-connect".into(), 10),
        Arc::new(AtomicBool::new(false)),
        &mut |_, _| {
            emitted = true;
            Ok(())
        },
        Instant::now() - Duration::from_secs(1),
    );
    assert_eq!(result, Err(ForwardError::Timeout));
    assert!(!emitted);
}

#[test]
fn expired_http_deadline_never_starts_dns_or_a_socket() {
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    let fetcher = http::DirectHttpFetcher::with_resolver(Arc::new(move |_| {
        observed.store(true, Ordering::Release);
        Box::pin(async { Ok(Vec::new()) })
    }));
    let result = fetcher.send_until(
        ::http::Method::GET,
        &url::Url::parse("https://expired.example/").unwrap(),
        &[],
        &[],
        Some(Instant::now() - Duration::from_millis(1)),
        Arc::new(AtomicBool::new(false)),
    );
    assert!(matches!(result, Err(fetch::FetchFailure::Timeout)));
    assert!(!called.load(Ordering::Acquire));
}
