//! Type 29 adapter for the shared direct HTTP transport. No request policy.
use super::http::DirectHttpFetcher;
use prost::Message;
use quadcortex::{commands::OutboundMessage, proto::cortex_protobuf_v2 as pa};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
};
use zeroize::{Zeroize, Zeroizing};

pub struct CloudResponse {
    pub deadline: Option<std::time::Instant>,
    pub cancel: Arc<AtomicBool>,
    pub messages: Vec<OutboundMessage>,
}
impl std::ops::Deref for CloudResponse {
    type Target = [OutboundMessage];
    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}
impl Drop for CloudResponse {
    fn drop(&mut self) {
        for message in &mut self.messages {
            message.payload.zeroize();
        }
    }
}
struct SecretResponse {
    deadline: Option<std::time::Instant>,
    epoch: u64,
    cancel: Arc<AtomicBool>,
    messages: Vec<OutboundMessage>,
}
impl Drop for SecretResponse {
    fn drop(&mut self) {
        for m in &mut self.messages {
            m.payload.zeroize();
        }
    }
}
struct Task {
    epoch: u64,
    cancel: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}
pub struct CloudForwardRelay {
    enabled: bool,
    next_response_request_id: u64,
    sender: Sender<SecretResponse>,
    replies: Receiver<SecretResponse>,
    tasks: Vec<Task>,
    response_cancellations: Vec<std::sync::Weak<AtomicBool>>,
    fetcher: std::sync::Arc<DirectHttpFetcher>,
}
impl Default for CloudForwardRelay {
    fn default() -> Self {
        Self::with_fetcher(DirectHttpFetcher::default())
    }
}
impl CloudForwardRelay {
    pub fn with_fetcher(fetcher: DirectHttpFetcher) -> Self {
        let (sender, replies) = mpsc::channel();
        Self {
            enabled: false,
            next_response_request_id: 8,
            sender,
            replies,
            tasks: Vec::new(),
            response_cancellations: Vec::new(),
            fetcher: std::sync::Arc::new(fetcher),
        }
    }
}
impl CloudForwardRelay {
    pub fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.cancel_all();
        }
        self.enabled = enabled;
    }
    /// Retire all requests immediately without joining on the USB owner thread.
    /// A cancelled response can never become current again after re-enabling.
    pub fn cancel_all(&mut self) {
        for cancel in self
            .response_cancellations
            .iter()
            .filter_map(std::sync::Weak::upgrade)
        {
            cancel.store(true, Ordering::Release);
        }
        self.response_cancellations
            .retain(|cancel| cancel.strong_count() > 0);
        for task in &self.tasks {
            task.cancel.store(true, Ordering::Release);
        }
        while let Ok(_reply) = self.replies.try_recv() {}
    }
    fn response_request_id(&mut self, incoming: Option<u64>) -> u64 {
        incoming.unwrap_or_else(|| {
            let id = self.next_response_request_id;
            self.next_response_request_id = self.next_response_request_id.wrapping_add(1);
            id
        })
    }
    pub fn on_cloud_login(&self, payload: &mut Vec<u8>) {
        // Type 18 is a device notification with no HTTP request to forward.
        payload.zeroize();
    }
    pub fn on_product_forward(&mut self, payload: &mut Vec<u8>, epoch: u64) {
        let decoded = pa::ProductForwardMessage::decode(payload.as_slice());
        payload.zeroize();
        if !self.enabled {
            return;
        }
        let Ok(mut message) = decoded else { return };
        let incoming = message
            .request_id
            .take()
            .map(|pa::product_forward_message::RequestId::RequestId(id)| id);
        let request_id = Some(self.response_request_id(incoming));
        let cloud_id = message.cloud_request_id;
        let Some(pa::product_forward_message::Request::Request(request)) = message.request.take()
        else {
            return;
        };
        let sender = self.sender.clone();
        let fetcher = std::sync::Arc::clone(&self.fetcher);
        let cancel = Arc::new(AtomicBool::new(false));
        self.response_cancellations
            .retain(|cancel| cancel.strong_count() > 0);
        self.response_cancellations.push(Arc::downgrade(&cancel));
        let task_cancel = Arc::clone(&cancel);
        let started = std::time::Instant::now();
        let failure_deadline = (request.timeout > 0)
            .then(|| started + std::time::Duration::from_millis(request.timeout as u64));
        let spawned = thread::Builder::new()
            .name("qc-http-product".into())
            .spawn(move || {
                let mut message = pa::ProductForwardMessage {
                    cloud_request_id: cloud_id,
                    request_id: request_id.map(pa::product_forward_message::RequestId::RequestId),
                    request: Some(pa::product_forward_message::Request::Request(request)),
                    ..Default::default()
                };
                let wire = Zeroizing::new(message.encode_to_vec());
                if let Some(pa::product_forward_message::Request::Request(request)) =
                    &mut message.request
                {
                    if let Some(pa::product_forward_request::Payload::Payload(bytes)) =
                        &mut request.payload
                    {
                        bytes.zeroize();
                    }
                    for header in &mut request.headers {
                        header.value.zeroize();
                    }
                    request.url.zeroize();
                }
                let mut messages = Vec::new();
                let mut deadline = None;
                let _ = crate::forward_with_fetcher_at(
                    &fetcher,
                    29,
                    &wire,
                    Arc::clone(&task_cancel),
                    &mut |message, limit| {
                        deadline = limit;
                        messages.push(message);
                        Ok(())
                    },
                    started,
                );
                if !task_cancel.load(Ordering::Acquire) {
                    let _ = sender.send(SecretResponse {
                        deadline,
                        epoch,
                        cancel: Arc::clone(&task_cancel),
                        messages,
                    });
                }
            });
        match spawned {
            Ok(thread) => self.tasks.push(Task {
                epoch,
                cancel,
                thread,
            }),
            Err(_) => {
                let _ = self.sender.send(SecretResponse {
                    deadline: failure_deadline,
                    epoch,
                    cancel: Arc::clone(&cancel),
                    messages: vec![product_forward_response(
                        request_id,
                        cloud_id,
                        Vec::new(),
                        503,
                    )],
                });
            }
        }
    }
    pub fn take_response(&mut self, epoch: u64) -> Option<CloudResponse> {
        for task in &self.tasks {
            if task.epoch != epoch {
                task.cancel.store(true, Ordering::Release);
            }
        }
        let mut i = 0;
        while i < self.tasks.len() {
            if self.tasks[i].thread.is_finished() {
                let task = self.tasks.swap_remove(i);
                let _ = task.thread.join();
            } else {
                i += 1;
            }
        }
        while let Ok(mut reply) = self.replies.try_recv() {
            if self.enabled
                && reply.epoch == epoch
                && !reply.cancel.load(Ordering::Acquire)
                && !reply
                    .deadline
                    .is_some_and(|end| std::time::Instant::now() >= end)
            {
                return Some(CloudResponse {
                    deadline: reply.deadline,
                    cancel: Arc::clone(&reply.cancel),
                    messages: std::mem::take(&mut reply.messages),
                });
            }
        }
        None
    }
}
impl Drop for CloudForwardRelay {
    fn drop(&mut self) {
        self.cancel_all();
        for task in self.tasks.drain(..) {
            let _ = task.thread.join();
        }
    }
}

pub fn product_forward_response(
    request_id: Option<u64>,
    cloud_request_id: u64,
    response_payload: Vec<u8>,
    error_code: i32,
) -> OutboundMessage {
    let mut response_payload = Zeroizing::new(response_payload);
    let mut response = pa::ProductForwardMessage {
        action: pa::message_action::Enum::Update as i32,
        request_id: request_id.map(pa::product_forward_message::RequestId::RequestId),
        cloud_request_id,
        response: Some(pa::product_forward_message::Response::Response(
            pa::ProductForwardResponse {
                payload: std::mem::take(&mut *response_payload),
                error_code,
            },
        )),
        ..Default::default()
    };
    let mut wire_payload = response.encode_to_vec();
    if let Some(pa::product_forward_message::Response::Response(response)) = &mut response.response
    {
        response.payload.zeroize();
    }
    response_payload.zeroize();
    let message = OutboundMessage {
        message_type: 29,
        payload: std::mem::take(&mut wire_payload),
    };
    wire_payload.zeroize();
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn product_forward_translates_only_the_envelope_and_preserves_large_error_body() {
        let response = vec![0xff; 600 * 1024];
        let (url, received, server) = super::super::http::wire_tests::serve(401, response.clone());
        let body = vec![0xab; 100 * 1024];
        let mut message = pa::ProductForwardMessage {
            action: 1,
            cloud_request_id: 55,
            request: Some(pa::product_forward_message::Request::Request(
                pa::ProductForwardRequest {
                    operation: 2,
                    url: url.as_str().into(),
                    timeout: 0,
                    is_multipart: true,
                    payload: Some(pa::product_forward_request::Payload::Payload(body.clone())),
                    headers: (0..50)
                        .map(|i| pa::ProductForwardHeaderPair {
                            name: format!("X-Device-{i}"),
                            value: "exact".into(),
                        })
                        .collect(),
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        relay.on_product_forward(&mut message, 7);
        assert!(message.iter().all(|b| *b == 0));
        let deadline = Instant::now() + Duration::from_secs(5);
        let replies = loop {
            if let Some(replies) = relay.take_response(7) {
                break replies;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(replies.len(), 1);
        let reply = pa::ProductForwardMessage::decode(replies[0].payload.as_slice()).unwrap();
        assert_eq!(reply.cloud_request_id, 55);
        assert!(matches!(
            reply.request_id,
            Some(pa::product_forward_message::RequestId::RequestId(8))
        ));
        let Some(pa::product_forward_message::Response::Response(actual)) = reply.response else {
            panic!("missing response")
        };
        assert_eq!(actual.error_code, 401);
        assert_eq!(actual.payload, response);
        let request = received.recv_timeout(Duration::from_secs(5)).unwrap();
        server.join().unwrap().unwrap();
        let end = request.windows(4).position(|s| s == b"\r\n\r\n").unwrap();
        assert_eq!(&request[end + 4..], body);
        assert!(String::from_utf8_lossy(&request[..end])
            .to_ascii_lowercase()
            .contains("x-device-49: exact"));
    }
    #[test]
    fn dropping_account_adapter_cancels_a_zero_timeout_exchange() {
        use std::{io::Read, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut block = [0; 4096];
            let _ = socket.read(&mut block);
            tx.send(()).unwrap();
            thread::sleep(Duration::from_millis(300));
        });
        let mut payload = pa::ProductForwardMessage {
            action: 1,
            cloud_request_id: 56,
            request: Some(pa::product_forward_message::Request::Request(
                pa::ProductForwardRequest {
                    operation: 0,
                    url: format!("http://{address}/"),
                    timeout: 0,
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        relay.on_product_forward(&mut payload, 7);
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        drop(relay);
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
    #[test]
    fn session_retirement_cancels_zero_timeout_socket_and_rejects_old_reply() {
        use std::{io::Read, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                assert_eq!(socket.read(&mut byte).unwrap(), 1);
                head.push(byte[0]);
            }
            ready_tx.send(()).unwrap();
            socket.read(&mut byte)
        });
        let mut payload = pa::ProductForwardMessage {
            cloud_request_id: 9,
            request: Some(pa::product_forward_message::Request::Request(
                pa::ProductForwardRequest {
                    operation: 0,
                    url: format!("http://{address}/"),
                    timeout: 0,
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        relay.on_product_forward(&mut payload, 3);
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        relay.cancel_all();
        assert!(started.elapsed() < Duration::from_millis(100));
        let closed = server.join().unwrap();
        assert!(
            matches!(closed, Ok(0))
                || matches!(closed, Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionReset)
        );
        relay.set_enabled(false);
        relay.set_enabled(true);
        assert!(relay.take_response(3).is_none());
        drop(relay);
    }

    #[test]
    fn delayed_drain_drops_a_completed_reply_after_its_absolute_deadline() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        let expired = std::time::Instant::now() - Duration::from_millis(1);
        relay
            .sender
            .send(SecretResponse {
                epoch: 3,
                deadline: Some(expired),
                cancel: Arc::new(AtomicBool::new(false)),
                messages: vec![product_forward_response(Some(8), 9, vec![1, 2, 3], 200)],
            })
            .ok()
            .unwrap();
        assert!(relay.take_response(3).is_none());
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        relay
            .sender
            .send(SecretResponse {
                epoch: 3,
                deadline: Some(deadline),
                cancel: Arc::new(AtomicBool::new(false)),
                messages: vec![product_forward_response(Some(8), 9, vec![1, 2, 3], 200)],
            })
            .ok()
            .unwrap();
        let response = relay.take_response(3).unwrap();
        assert_eq!(response.deadline, Some(deadline));
        assert_eq!(response.len(), 1);
    }

    #[test]
    fn disable_cancels_a_reply_already_transferred_to_usb_ownership() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        let cancel = Arc::new(AtomicBool::new(false));
        relay.response_cancellations.push(Arc::downgrade(&cancel));
        relay
            .sender
            .send(SecretResponse {
                epoch: 3,
                deadline: None,
                cancel: Arc::clone(&cancel),
                messages: vec![product_forward_response(Some(8), 9, vec![1], 200)],
            })
            .ok()
            .unwrap();
        let response = relay.take_response(3).unwrap();
        assert!(relay.tasks.is_empty());
        relay.set_enabled(false);
        assert!(response.cancel.load(Ordering::Acquire));
        relay.set_enabled(true);
        assert!(response.cancel.load(Ordering::Acquire));
    }
}
