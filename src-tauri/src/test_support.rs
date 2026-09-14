//! Deterministic in-process HTTP test infrastructure.
//!
//! This module exists only for tests: it serves fixed scripted responses on an
//! ephemeral loopback port so acquisition tests never depend on public
//! internet access. It is intentionally a minimal hand-rolled HTTP/1.1
//! responder, not a web framework.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

/// A scripted request received by a [`TestServer`].
#[derive(Debug, Clone)]
pub struct TestRequest {
    pub path: String,
    /// `http://127.0.0.1:<port>` for the server handling this request.
    pub base_url: String,
}

/// A scripted response served by a [`TestServer`].
#[derive(Debug, Clone)]
pub struct TestResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Send only this many body bytes, then close the socket abruptly,
    /// simulating a truncated transfer.
    pub truncate_body_to: Option<usize>,
    /// Delay between small body writes, simulating a slow host.
    pub drip_delay: Option<Duration>,
    /// Omit Content-Length and close-delimit the body instead.
    pub connection_close_framing: bool,
}

impl TestResponse {
    pub fn ok(body: &[u8]) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.to_vec(),
            truncate_body_to: None,
            drip_delay: None,
            connection_close_framing: false,
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::ok(&[])
        }
    }

    /// `302 Found` redirecting to an absolute URL.
    pub fn redirect_to(location: String) -> Self {
        Self {
            status: 302,
            headers: vec![("Location".to_owned(), location)],
            ..Self::ok(&[])
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn with_truncated_body(mut self, kept_bytes: usize) -> Self {
        self.truncate_body_to = Some(kept_bytes);
        self
    }

    pub fn with_drip_delay(mut self, delay: Duration) -> Self {
        self.drip_delay = Some(delay);
        self
    }

    pub fn with_close_framing(mut self) -> Self {
        self.connection_close_framing = true;
        self
    }
}

type Handler = dyn Fn(&TestRequest) -> TestResponse + Send + Sync + 'static;

/// A loopback HTTP server running on an ephemeral port in background threads.
///
/// Dropping the server closes the listener.
pub struct TestServer {
    base_url: String,
    requests: Arc<AtomicUsize>,
    _listener: TcpListener,
}

impl TestServer {
    /// Starts a server that answers every connection by calling `handler`.
    pub fn spawn(handler: Arc<Handler>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind must succeed");
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));

        let accept_listener = listener
            .try_clone()
            .expect("listener handles must be cloneable on test platforms");
        let accept_requests = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in accept_listener.incoming() {
                let Ok(stream) = stream else {
                    break;
                };
                let handler = Arc::clone(&handler);
                let served = Arc::clone(&accept_requests);
                thread::spawn(move || {
                    let _ = serve_connection(stream, &*handler, &served);
                });
            }
        });

        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            requests,
            _listener: listener,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// How many connections the server has accepted so far.
    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

fn serve_connection(
    mut stream: std::net::TcpStream,
    handler: &Handler,
    requests: &AtomicUsize,
) -> std::io::Result<()> {
    let request = read_request(&mut stream)?;
    requests.fetch_add(1, Ordering::SeqCst);
    let response = handler(&request);
    write_response(&mut stream, &response)
}

/// Reads one request head. Only the request line is interpreted; test
/// downloads never send bodies.
fn read_request(stream: &mut std::net::TcpStream) -> std::io::Result<TestRequest> {
    let base_url = format!("http://{}", stream.local_addr()?.to_string());

    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "client closed before sending a request",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
    }

    let head = String::from_utf8_lossy(&buffer);
    let mut parts = head.split_whitespace();
    let _method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default().to_owned();

    Ok(TestRequest { path, base_url })
}

fn write_response(
    stream: &mut std::net::TcpStream,
    response: &TestResponse,
) -> std::io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        302 => "Found",
        404 => "Not Found",
        _ => "Response",
    };

    let mut head = format!("HTTP/1.1 {} {}\r\n", response.status, reason);
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !response.connection_close_framing {
        head.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;

    let body_limit = response
        .truncate_body_to
        .unwrap_or(response.body.len())
        .min(response.body.len());

    match response.drip_delay {
        None => stream.write_all(&response.body[..body_limit])?,
        Some(delay) => {
            let drip = 3usize;
            for chunk in response.body[..body_limit].chunks(drip.max(1)) {
                stream.write_all(chunk)?;
                stream.flush()?;
                thread::sleep(delay);
            }
        }
    }
    stream.flush()
}
