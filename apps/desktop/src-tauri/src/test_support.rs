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
