//! Test-only helpers: a tiny HTTP/1.1 server with Range support.
//!
//! The desktop tests need a real download endpoint to exercise Range resume,
//! pausing and repair. Pulling a web framework in just for tests would add a
//! large dependency tree, so this speaks the small subset of HTTP that
//! `reqwest` uses for a single GET.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct TestServer {
    pub url: String,
    /// Range header of every request, in order (`""` when absent).
    pub ranges: Arc<Mutex<Vec<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub async fn serve_bytes(payload: Vec<u8>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let payload = Arc::new(payload);
    let recorded = Arc::clone(&ranges);

    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let payload = Arc::clone(&payload);
            let ranges = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 16 * 1024];
                let mut read = 0usize;
                loop {
                    if read == buffer.len() {
                        return;
                    }
                    match socket.read(&mut buffer[read..]).await {
                        Ok(0) => return,
                        Ok(bytes) => {
                            read += bytes;
                            if buffer[..read]
                                .windows(4)
                                .any(|window| window == b"\r\n\r\n")
                            {
                                break;
                            }
                        }
                        Err(_) => return,
                    }
                }

                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let mut range = String::new();
                for line in request.lines() {
                    if line.to_ascii_lowercase().starts_with("range:") {
                        if let Some((_, value)) = line.split_once(':') {
                            range = value.trim().to_string();
                        }
                    }
                }
                if let Ok(mut ranges) = ranges.lock() {
                    ranges.push(range.clone());
                }

                let total = payload.len() as u64;
                let (start, end, status) = match range.strip_prefix("bytes=") {
                    Some(spec) => {
                        let spec = spec.split(',').next().unwrap_or_default();
                        let (from, to) = spec.split_once('-').unwrap_or(("", ""));
                        let start = from.parse::<u64>().unwrap_or(0).min(total);
                        let end = to
                            .parse::<u64>()
                            .ok()
                            .map(|value| value.min(total.saturating_sub(1)))
                            .unwrap_or_else(|| total.saturating_sub(1));
                        if start > end || start >= total {
                            (0, 0, 416)
                        } else {
                            (start, end, 206)
                        }
                    }
                    None => (0, total.saturating_sub(1), 200),
                };

                let body: Vec<u8> = if status == 416 || total == 0 {
                    Vec::new()
                } else {
                    payload[start as usize..=end as usize].to_vec()
                };
                let head = if status == 416 {
                    format!(
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else if status == 206 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                };

                let mut response = head.into_bytes();
                response.extend_from_slice(&body);
                let _ = socket.write_all(&response).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    TestServer {
        url: format!("http://{address}/archive"),
        ranges,
        handle,
    }
}

/// A tiny server exposing a stream manifest and one ranged endpoint per entry,
/// mirroring the server routes from #115.
pub struct ZipServer {
    /// Base URL; the client appends `/api/games/…`, which is ignored here.
    pub url: String,
    /// `(entry index, Range header)` of every entry request, in order.
    pub ranges: Arc<Mutex<Vec<(usize, String)>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for ZipServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Serves `manifest` and the raw bytes of every entry.
///
/// `entries` is indexed by manifest position (not central-directory index);
/// directory entries are ignored. `fail_entry` makes one entry answer 500 so a
/// test can interrupt a run and resume it.
pub async fn serve_zip(
    manifest: ludex_core::archive::StreamManifest,
    entries: Vec<Vec<u8>>,
    fail_entry: Option<usize>,
) -> ZipServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&ranges);
    let manifest = Arc::new(serde_json::to_vec(&manifest).expect("manifest json"));
    let entries = Arc::new(entries);

    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let manifest = Arc::clone(&manifest);
            let entries = Arc::clone(&entries);
            let ranges = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 16 * 1024];
                let mut read = 0usize;
                loop {
                    if read == buffer.len() {
                        return;
                    }
                    match socket.read(&mut buffer[read..]).await {
                        Ok(0) => return,
                        Ok(bytes) => {
                            read += bytes;
                            if buffer[..read]
                                .windows(4)
                                .any(|window| window == b"\r\n\r\n")
                            {
                                break;
                            }
                        }
                        Err(_) => return,
                    }
                }

                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                let mut range = String::new();
                for line in request.lines() {
                    if line.to_ascii_lowercase().starts_with("range:") {
                        if let Some((_, value)) = line.split_once(':') {
                            range = value.trim().to_string();
                        }
                    }
                }

                if path.ends_with("/stream/manifest") {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        manifest.len()
                    );
                    let mut response = head.into_bytes();
                    response.extend_from_slice(&manifest);
                    let _ = socket.write_all(&response).await;
                    let _ = socket.shutdown().await;
                    return;
                }

                let index = path
                    .rsplit('/')
                    .next()
                    .and_then(|value| value.parse::<usize>().ok());
                let Some(index) = index else {
                    let _ = socket
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    return;
                };
                if let Ok(mut ranges) = ranges.lock() {
                    ranges.push((index, range.clone()));
                }
                if fail_entry == Some(index) {
                    let _ = socket
                        .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    return;
                }
                let Some(payload) = entries.get(index) else {
                    let _ = socket
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    return;
                };
                let total = payload.len() as u64;
                let (start, end, status) = match range.strip_prefix("bytes=") {
                    Some(spec) => {
                        let spec = spec.split(',').next().unwrap_or_default();
                        let (from, to) = spec.split_once('-').unwrap_or(("", ""));
                        let start = from.parse::<u64>().unwrap_or(0).min(total);
                        let end = to
                            .parse::<u64>()
                            .ok()
                            .map(|value| value.min(total.saturating_sub(1)))
                            .unwrap_or_else(|| total.saturating_sub(1));
                        if start > end || start >= total {
                            (0, 0, 416)
                        } else {
                            (start, end, 206)
                        }
                    }
                    None => (0, total.saturating_sub(1), 200),
                };
                let body: Vec<u8> = if status == 416 || total == 0 {
                    Vec::new()
                } else {
                    payload[start as usize..=end as usize].to_vec()
                };
                let head = if status == 416 {
                    format!(
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else if status == 206 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                };
                let mut response = head.into_bytes();
                response.extend_from_slice(&body);
                let _ = socket.write_all(&response).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    ZipServer {
        url: format!("http://{address}"),
        ranges,
        handle,
    }
}

/// A tiny scripted JSON server for the SSO handoff tests (#157).
pub struct JsonServer {
    /// Base URL; callers append the API path they want to hit.
    pub url: String,
    /// Parsed JSON body of every request, in order.
    pub requests: Arc<Mutex<Vec<serde_json::Value>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for JsonServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Serves the given `(status, body)` responses in order; the last one repeats.
///
/// Every request body is parsed as JSON and recorded, so a test can assert on
/// what the client actually sent.
pub async fn serve_json(responses: Vec<(u16, serde_json::Value)>) -> JsonServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let responses = Arc::new(responses);
    let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let requests = Arc::clone(&recorded);
            let responses = Arc::clone(&responses);
            let next = Arc::clone(&next);
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 32 * 1024];
                let mut read = 0usize;
                let header_end = loop {
                    if read == buffer.len() {
                        return;
                    }
                    match socket.read(&mut buffer[read..]).await {
                        Ok(0) => return,
                        Ok(bytes) => {
                            read += bytes;
                            if let Some(position) = buffer[..read]
                                .windows(4)
                                .position(|window| window == b"\r\n\r\n")
                            {
                                break position + 4;
                            }
                        }
                        Err(_) => return,
                    }
                };

                let request = String::from_utf8_lossy(&buffer[..header_end]).to_string();
                let content_length = request
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0)
                    .min(buffer.len() - header_end);
                while read < header_end + content_length {
                    match socket
                        .read(&mut buffer[read..header_end + content_length])
                        .await
                    {
                        Ok(0) | Err(_) => break,
                        Ok(bytes) => read += bytes,
                    }
                }
                let body = &buffer[header_end..read];
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
                    if let Ok(mut requests) = requests.lock() {
                        requests.push(value);
                    }
                }

                let index = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let (status, payload) = responses
                    .get(index)
                    .or_else(|| responses.last())
                    .cloned()
                    .unwrap_or((500, serde_json::json!({})));
                let body = serde_json::to_vec(&payload).unwrap_or_default();
                let reason = match status {
                    200 => "OK",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    410 => "Gone",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    _ => "Status",
                };
                let head = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let mut response = head.into_bytes();
                response.extend_from_slice(&body);
                let _ = socket.write_all(&response).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    JsonServer {
        url: format!("http://{address}"),
        requests,
        handle,
    }
}
