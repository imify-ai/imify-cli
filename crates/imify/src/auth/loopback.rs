//! The listener that receives the authorization redirect on `localhost`.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use reqwest::Url;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::{Error, LoginFailure};

/// Path of the redirect URI.
pub(crate) const CALLBACK_PATH: &str = "/callback";

/// Longest request head the listener reads.
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// How long one connection may take to send its request head. Browsers open connections
/// they never use, so a silent connection must not block the others.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Pause after a failed `accept`, so a persistent failure does not spin.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// The query of an authorization redirect. `Debug` redacts the code and `state`.
pub(crate) enum Callback {
    /// The provider issued an authorization code.
    Code { code: String, state: String },
    /// The provider reported an error, such as `access_denied`.
    Error {
        error: String,
        description: Option<String>,
    },
}

impl fmt::Debug for Callback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code { .. } => formatter.write_str("Callback::Code(<redacted>)"),
            Self::Error { error, description } => formatter
                .debug_struct("Callback::Error")
                .field("error", error)
                .field("description", description)
                .finish(),
        }
    }
}

/// Reads `code` and `state`, or `error`, from the query of `url`. `None` when the query
/// has neither.
pub(crate) fn parse_callback(url: &Url) -> Option<Callback> {
    let (mut code, mut state, mut error, mut description) = (None, None, None, None);
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            "error_description" => description = Some(value.into_owned()),
            _ => {}
        }
    }
    match (error, code, state) {
        (Some(error), _, _) => Some(Callback::Error { error, description }),
        (None, Some(code), Some(state)) => Some(Callback::Code { code, state }),
        _ => None,
    }
}

/// Listens on `127.0.0.1` and, when available, `::1`, on the first port that can be bound.
#[derive(Debug)]
pub(crate) struct RedirectListener {
    listeners: Vec<TcpListener>,
    port: u16,
}

impl RedirectListener {
    /// Binds the first port of `ports` that is free on `127.0.0.1`.
    ///
    /// Fails with [`LoginFailure::PortsInUse`] when none is.
    pub(crate) async fn bind(ports: &[u16]) -> Result<Self, Error> {
        for &port in ports {
            let Ok(ipv4) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await else {
                continue;
            };
            let mut listeners = vec![ipv4];
            if let Ok(ipv6) = TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
                listeners.push(ipv6);
            }
            return Ok(Self { listeners, port });
        }
        let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
        Err(Error::Login {
            reason: LoginFailure::PortsInUse,
            message: format!(
                "none of the redirect ports {} can be bound on 127.0.0.1",
                ports.join(", ")
            ),
        })
    }

    /// The bound port.
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Waits for the first request to the callback path that carries a code or an error,
    /// answering every request with a short HTML page. Requests to other paths get 404.
    ///
    /// Fails with [`LoginFailure::TimedOut`] when none arrives within `timeout`.
    pub(crate) async fn wait(self, timeout: Duration) -> Result<Callback, Error> {
        let (sender, mut callbacks) = mpsc::channel(1);
        let mut accept_loops = JoinSet::new();
        for listener in self.listeners {
            accept_loops.spawn(accept_loop(listener, sender.clone()));
        }
        drop(sender);
        let received = tokio::time::timeout(timeout, callbacks.recv()).await;
        accept_loops.abort_all();
        match received {
            Ok(Some(callback)) => Ok(callback),
            Ok(None) | Err(_) => Err(Error::Login {
                reason: LoginFailure::TimedOut,
                message: format!(
                    "no redirect from the browser arrived within {} s",
                    timeout.as_secs()
                ),
            }),
        }
    }
}

async fn accept_loop(listener: TcpListener, sender: mpsc::Sender<Callback>) {
    let mut connections = JoinSet::new();
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                connections.spawn(serve(stream, sender.clone()));
            }
            Err(_) => tokio::time::sleep(ACCEPT_RETRY_DELAY).await,
        }
        while connections.try_join_next().is_some() {}
    }
}

/// Answers one connection and forwards its callback, if any.
async fn serve(mut stream: TcpStream, sender: mpsc::Sender<Callback>) {
    let Ok(Some(head)) = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut stream)).await else {
        respond(&mut stream, Response::BAD_REQUEST).await;
        return;
    };
    match route(&head) {
        Route::Callback(callback) => {
            let response = match callback {
                Callback::Code { .. } => Response::RECEIVED,
                Callback::Error { .. } => Response::FAILED,
            };
            respond(&mut stream, response).await;
            // Only the first callback is awaited; later ones are dropped.
            let _ = sender.try_send(callback);
        }
        Route::Rejected(response) => respond(&mut stream, response).await,
    }
}

/// Reads until the end of the request head. `None` when the connection closes first or
/// the head exceeds [`MAX_HEAD_BYTES`].
async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 || head.len() + read > MAX_HEAD_BYTES {
            return None;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(head).ok()
}

enum Route {
    Callback(Callback),
    Rejected(Response),
}

fn route(head: &str) -> Route {
    let mut request_line = head.lines().next().unwrap_or_default().split(' ');
    let (Some(method), Some(target)) = (request_line.next(), request_line.next()) else {
        return Route::Rejected(Response::BAD_REQUEST);
    };
    let Ok(url) = Url::parse(&format!("http://localhost{target}")) else {
        return Route::Rejected(Response::BAD_REQUEST);
    };
    if url.path() != CALLBACK_PATH {
        return Route::Rejected(Response::NOT_FOUND);
    }
    if method != "GET" {
        return Route::Rejected(Response::METHOD_NOT_ALLOWED);
    }
    parse_callback(&url).map_or(Route::Rejected(Response::BAD_REQUEST), Route::Callback)
}

#[derive(Debug, Clone, Copy)]
struct Response {
    status: &'static str,
    message: &'static str,
}

impl Response {
    const RECEIVED: Self = Self {
        status: "200 OK",
        message: "Login received. Close this tab and return to the terminal.",
    };
    const FAILED: Self = Self {
        status: "200 OK",
        message: "Login failed. The terminal shows the reason.",
    };
    const BAD_REQUEST: Self = Self {
        status: "400 Bad Request",
        message: "The request is not an authorization redirect.",
    };
    const NOT_FOUND: Self = Self {
        status: "404 Not Found",
        message: "Not found.",
    };
    const METHOD_NOT_ALLOWED: Self = Self {
        status: "405 Method Not Allowed",
        message: "Only GET is allowed.",
    };
}

async fn respond(stream: &mut TcpStream, response: Response) {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>imify</title></head>\
         <body><p>{}</p></body></html>",
        response.message
    );
    let message = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        response.status,
        body.len()
    );
    // The browser may have closed the connection; the callback is handled either way.
    let _ = stream.write_all(message.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests;
