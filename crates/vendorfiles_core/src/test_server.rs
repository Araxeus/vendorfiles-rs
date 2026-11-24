//! A local HTTP server for tests that need real statuses, redirects and headers.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One canned answer, chosen by exact request target (query included).
pub struct Route {
    pub path: &'static str,
    pub status: u16,
    /// Extra response headers, each ending in `\r\n`.
    pub headers: &'static str,
    pub body: Vec<u8>,
}

pub fn route(path: &'static str, status: u16, headers: &'static str, body: &[u8]) -> Route {
    Route {
        path,
        status,
        headers,
        body: body.to_vec(),
    }
}

/// Serves `routes` until the test process ends and returns the base URL. Unknown targets get a
/// 404.
pub fn serve(routes: Vec<Route>) -> String {
    serve_counted(routes).0
}

/// As [`serve`], with a count of the requests received.
pub fn serve_counted(routes: Vec<Route>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().expect("a bound address").port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    std::thread::spawn(move || {
        for mut socket in listener.incoming().flatten() {
            counter.fetch_add(1, Ordering::SeqCst);
            let target = request_target(&mut socket);
            let (status, headers, body) = routes
                .iter()
                .find(|r| r.path == target)
                .map_or((404, "", &[][..]), |r| (r.status, r.headers, &r.body[..]));
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                body.len()
            );
            let _ = socket.write_all(head.as_bytes());
            let _ = socket.write_all(body);
        }
    });
    (format!("http://127.0.0.1:{port}"), hits)
}

/// Accepts connections and never answers, and returns the base URL.
pub fn serve_silence() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().expect("a bound address").port();
    std::thread::spawn(move || {
        // Held, not dropped: a closed socket would answer with an error.
        let _held: Vec<TcpStream> = listener.incoming().flatten().collect();
    });
    format!("http://127.0.0.1:{port}")
}

/// Reads the request head and returns its target, like `/a/b?c=d`.
fn request_target(socket: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut buffer = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match socket.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buffer[..n]),
        }
    }
    String::from_utf8_lossy(&head)
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned()
}
