//! The loopback OAuth redirect receiver.
//!
//! One login transaction binds one listener on `127.0.0.1` at an ephemeral
//! port; the authorization request tells Microsoft to redirect the system
//! browser to `http://localhost:<port>/` (the port is ignored for matching by
//! the Microsoft identity platform, so an ephemeral port works with a plain
//! registered `http://localhost` redirect, while the path is matched exactly
//! — the registration's root path is what the URI must carry).
//!
//! The receiver is local-only, accepts exactly the registered root path,
//! ignores callbacks whose state does not match this transaction (stale tabs
//! must not kill or hijack a fresh login), stops at the first decisive
//! callback, and disappears with its listener when the flow ends — by
//! success, provider error, cancellation, or timeout.

use std::fmt;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::oauth::{self, CallbackQuery};

/// The only path accepted as a callback: the root path of the registered
/// loopback redirect (`http://localhost`). Loopback matching ignores the
/// port but compares the path exactly, so no path segment may be appended.
pub const CALLBACK_PATH: &str = "/";

/// Bound on one request head (callback requests carry query strings only).
const MAX_REQUEST_HEAD_BYTES: usize = 8 * 1024;

/// Bound on reading one browser connection.
const CONNECTION_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The fixed page shown after a decisive callback. It never reflects request
/// data back to the browser.
const COMPLETION_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Aurora Launcher</title></head><body style=\"font-family:system-ui;background:#111524;color:#eef4ff;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h1>Sign-in complete</h1><p>You can return to Aurora Launcher and close this tab.</p></div></body></html>";

const REJECTION_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\"></head><body style=\"font-family:system-ui;background:#111524;color:#eef4ff;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h1>Sign-in could not complete</h1><p>Please return to Aurora Launcher and try again.</p></div></body></html>";

/// Failures of the receiver itself (not provider-declared OAuth errors).
#[derive(Debug)]
pub enum CallbackReceiverError {
    /// The provider redirected with an OAuth error (for example the user
    /// declined consent or cancelled in the browser).
    ProviderError {
        error: String,
        description: Option<String>,
    },
    /// A callback arrived on the right path but carried neither a code nor
    /// an error.
    Invalid,
    /// The listener failed or was closed before a decisive callback.
    Listener(String),
}

impl fmt::Display for CallbackReceiverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderError { error, description } => write!(
                formatter,
                "Microsoft returned the sign-in error '{error}'{}",
                description
                    .as_deref()
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default()
            ),
            Self::Invalid => write!(
                formatter,
                "the sign-in callback was malformed: it carried neither an authorization code nor an error"
            ),
            Self::Listener(reason) => {
                write!(formatter, "the local sign-in receiver failed: {reason}")
            }
        }
    }
}

impl std::error::Error for CallbackReceiverError {}

/// The local receiver for one login transaction.
pub struct CallbackReceiver {
    listener: TcpListener,
    port: u16,
}

impl CallbackReceiver {
    /// Binds the local-only listener on an ephemeral port.
    pub async fn bind() -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The redirect URI sent in the authorization request: the registered
    /// `localhost` hostname with the dynamically selected port and the
    /// registration's root path. The bind address (`127.0.0.1`) never
    /// enters the URI.
    pub fn redirect_uri(&self) -> String {
        format!("http://localhost:{}/", self.port)
    }

    /// Waits for the decisive callback.
    ///
    /// Mismatched-state callbacks are answered with the rejection page and
    /// ignored (they are stale tabs or unrelated requests, never this
    /// transaction's result). Non-callback paths (favicons, probes) are
    /// answered with 404 and ignored. The listener stops at the first
    /// decisive callback; dropping the returned future closes the listener.
    pub async fn wait_for_callback(
        self,
        expected_state: &str,
    ) -> Result<String, CallbackReceiverError> {
        loop {
            let (stream, _peer) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    return Err(CallbackReceiverError::Listener(error.to_string()));
                }
            };

            match handle_connection(stream, expected_state).await {
                ConnectionOutcome::Continue => continue,
                ConnectionOutcome::Code(code) => return Ok(code),
                ConnectionOutcome::ProviderError { error, description } => {
                    return Err(CallbackReceiverError::ProviderError { error, description });
                }
                ConnectionOutcome::Invalid => return Err(CallbackReceiverError::Invalid),
            }
        }
    }
}

enum ConnectionOutcome {
    /// Answered and not decisive; keep listening.
    Continue,
    Code(String),
    ProviderError {
        error: String,
        description: Option<String>,
    },
    Invalid,
}

async fn handle_connection(mut stream: TcpStream, expected_state: &str) -> ConnectionOutcome {
    let head = match read_request_head(&mut stream).await {
        Ok(head) => head,
        Err(_) => return ConnectionOutcome::Continue,
    };

    let mut parts = head.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();

    // Only an exact GET on the registered root path is a callback; every
    // other path (favicon requests, probes) is refused and ignored. The
    // comparison is on the path alone — a plain prefix test would match
    // every possible request target.
    let path = target
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(target);
    if method != "GET" || path != CALLBACK_PATH {
        let _ = respond(&mut stream, 404, "Not Found", REJECTION_PAGE).await;
        return ConnectionOutcome::Continue;
    }

    let query = target.split_once('?').map(|(_, query)| query).unwrap_or("");
    match oauth::parse_callback_query(query) {
        Some(CallbackQuery::Code { code, state }) => {
            if state != expected_state {
                // A stale tab or foreign request: never accepted, never
                // fatal for the waiting transaction.
                eprintln!("[aurora-launcher] ignored a sign-in callback with a mismatched state");
                let _ = respond(&mut stream, 400, "Bad Request", REJECTION_PAGE).await;
                return ConnectionOutcome::Continue;
            }

            let _ = respond(&mut stream, 200, "OK", COMPLETION_PAGE).await;
            ConnectionOutcome::Code(code)
        }
        Some(CallbackQuery::Failure {
            error,
            error_description,
        }) => {
            let _ = respond(&mut stream, 400, "Bad Request", REJECTION_PAGE).await;
            ConnectionOutcome::ProviderError {
                error,
                description: error_description,
            }
        }
        None => {
            let _ = respond(&mut stream, 400, "Bad Request", REJECTION_PAGE).await;
            ConnectionOutcome::Invalid
        }
    }
}

async fn read_request_head(stream: &mut TcpStream) -> Result<String, std::io::Error> {
    let read = tokio::time::timeout(CONNECTION_READ_TIMEOUT, async {
        let mut buffer = Vec::with_capacity(1024);
        let mut chunk = [0u8; 512];
        loop {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "browser closed before sending a request",
                ));
            }
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
            if buffer.len() > MAX_REQUEST_HEAD_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "request head too large",
                ));
            }
        }
        Ok(buffer)
    })
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "callback connection timed out",
        )
    })??;

    Ok(String::from_utf8_lossy(&read).into_owned())
}

async fn respond(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) -> Result<(), std::io::Error> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn send_raw(port: u16, raw: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("receiver must be reachable");
        stream.write_all(raw.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        String::from_utf8_lossy(&response).into_owned()
    }

    #[tokio::test]
    async fn a_valid_callback_delivers_the_code_and_closes_the_listener() {
        let receiver = CallbackReceiver::bind().await.unwrap();
        let port = receiver.port();
        assert_eq!(receiver.redirect_uri(), format!("http://localhost:{port}/"));

        let waiter =
            tokio::spawn(async move { receiver.wait_for_callback("expected-state").await });

        // A mismatched-state callback never satisfies the receiver…
        let stale = send_raw(
            port,
            "GET /?code=STALE-CODE&state=wrong-state HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(stale.starts_with("HTTP/1.1 400"));

        // …and neither does an unrelated path — including the path shape a
        // non-root registration would use, which this registration forbids.
        let favicon = send_raw(port, "GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert!(favicon.starts_with("HTTP/1.1 404"));
        let non_root = send_raw(
            port,
            "GET /callback?code=STRAY-CODE&state=expected-state HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(non_root.starts_with("HTTP/1.1 404"));

        // The correct state completes the transaction.
        let done = send_raw(
            port,
            "GET /?code=FIXTURE-AUTH-CODE&state=expected-state HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(done.starts_with("HTTP/1.1 200"));
        assert!(done.contains("Sign-in complete"));

        let code = waiter
            .await
            .expect("waiter task completes")
            .expect("callback succeeds");
        assert_eq!(code, "FIXTURE-AUTH-CODE");

        // The listener is gone with the transaction.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let refused = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
        assert!(refused.is_err(), "listener must be closed after completion");
    }

    #[tokio::test]
    async fn the_listener_is_loopback_only_and_advertises_the_registered_localhost_root() {
        let receiver = CallbackReceiver::bind().await.unwrap();

        // The listener accepts connections only on the loopback interface.
        let address = receiver.listener.local_addr().unwrap();
        assert!(address.ip().is_loopback());

        // The advertised redirect URI carries the registered localhost
        // hostname, the dynamically selected port, and the registration's
        // root path — never the bind address or a path segment.
        let port = address.port();
        assert_ne!(port, 0, "the OS must have selected a concrete port");
        let uri = receiver.redirect_uri();
        assert_eq!(uri, format!("http://localhost:{port}/"));
        assert!(
            !uri.contains("127.0.0.1"),
            "the bind address never enters the URI"
        );
        assert_eq!(
            uri.strip_prefix("http://localhost:").unwrap_or(""),
            format!("{port}/"),
            "beyond host and dynamic port there is only the root path"
        );
    }

    #[tokio::test]
    async fn a_provider_error_callback_fails_the_flow_with_its_reason() {
        let receiver = CallbackReceiver::bind().await.unwrap();
        let port = receiver.port();
        let waiter =
            tokio::spawn(async move { receiver.wait_for_callback("expected-state").await });

        let response = send_raw(
            port,
            "GET /?error=access_denied&error_description=user+declined HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400"));

        let error = waiter
            .await
            .expect("waiter task completes")
            .expect_err("provider errors fail the flow");
        assert!(matches!(
            error,
            CallbackReceiverError::ProviderError { ref error, ref description }
                if error == "access_denied"
                && description.as_deref() == Some("user declined")
        ));
    }

    #[tokio::test]
    async fn a_codeless_callback_on_the_right_path_is_invalid() {
        let receiver = CallbackReceiver::bind().await.unwrap();
        let port = receiver.port();
        let waiter =
            tokio::spawn(async move { receiver.wait_for_callback("expected-state").await });

        let response = send_raw(
            port,
            "GET /?state=expected-state&foo=bar HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400"));

        let error = waiter
            .await
            .expect("waiter task completes")
            .expect_err("malformed callbacks fail the flow");
        assert!(matches!(error, CallbackReceiverError::Invalid));
    }

    #[tokio::test]
    async fn dropping_the_waiter_closes_the_listener() {
        let receiver = CallbackReceiver::bind().await.unwrap();
        let port = receiver.port();
        let waiter =
            tokio::spawn(async move { receiver.wait_for_callback("expected-state").await });
        waiter.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let refused = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
        assert!(
            refused.is_err(),
            "aborting the wait must close the listener"
        );
    }
}
