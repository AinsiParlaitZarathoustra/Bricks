//! Test helpers shared by the workspace's integration tests.
//!
//! [`TestServer`] is a scripted HTTP/1.1 server on 127.0.0.1: every request
//! goes to a handler that returns a [`Reply`]. Replies can stream their body
//! in delayed chunks, omit or falsify `Content-Length`, and the server counts
//! how many requests are in flight (overall and per path) so tests can prove
//! that work overlapped — or did not exceed a limit — without timing
//! thresholds. One request per connection (`Connection: close`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A request as received.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// Path and query, as sent.
    pub target: String,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    pub fn query(&self, key: &str) -> Option<String> {
        let q = self.target.split_once('?')?.1;
        q.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == key).then(|| percent_decode(v))
        })
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// How the body length is announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    /// `Content-Length` equal to the body length.
    Exact,
    /// No `Content-Length`: the body ends when the connection closes.
    None,
    /// A `Content-Length` that lies.
    Wrong(usize),
}

/// A scripted response.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Sent in order; each chunk after waiting its delay.
    pub chunks: Vec<(Duration, Vec<u8>)>,
    pub length: Length,
    /// Wait before sending the status line.
    pub delay: Duration,
}

impl Reply {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            chunks: Vec::new(),
            length: Length::Exact,
            delay: Duration::ZERO,
        }
    }

    pub fn text(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self::new(status)
            .header("content-type", content_type)
            .body(body)
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Self::text(200, "text/html; charset=utf-8", body)
    }

    pub fn json(status: u16, body: &str) -> Self {
        Self::text(status, "application/json", body.as_bytes().to_vec())
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Self::new(status).header("location", location)
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.chunks = vec![(Duration::ZERO, body.into())];
        self
    }

    /// Append a chunk sent after `delay`.
    pub fn chunk(mut self, delay: Duration, bytes: impl Into<Vec<u8>>) -> Self {
        self.chunks.push((delay, bytes.into()));
        self
    }

    pub fn length(mut self, length: Length) -> Self {
        self.length = length;
        self
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

pub type Handler = Arc<dyn Fn(Request) -> BoxFuture<'static, Reply> + Send + Sync>;

#[derive(Default)]
struct Counters {
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    per_path: Mutex<HashMap<String, (usize, usize)>>,
    total: AtomicUsize,
}

/// A scripted server; stops when dropped.
pub struct TestServer {
    pub addr: SocketAddr,
    counters: Arc<Counters>,
    requests: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    /// Serve `handler` on an ephemeral port of 127.0.0.1.
    pub async fn start<F, Fut>(handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Reply> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |r| Box::pin(handler(r)));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let counters = Arc::new(Counters::default());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (c, r) = (counters.clone(), requests.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let (h, c, r) = (handler.clone(), c.clone(), r.clone());
                tokio::spawn(async move {
                    let _ = serve(stream, h, c, r).await;
                });
            }
        });
        Self {
            addr,
            counters,
            requests,
            task,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// Highest number of requests handled at the same time.
    pub fn max_in_flight(&self) -> usize {
        self.counters.max_in_flight.load(Ordering::SeqCst)
    }

    /// Highest number of simultaneous requests whose path starts with `prefix`.
    pub fn max_in_flight_for(&self, prefix: &str) -> usize {
        self.counters
            .per_path
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(_, (_, max))| *max)
            .max()
            .unwrap_or(0)
    }

    pub fn total_requests(&self) -> usize {
        self.counters.total.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct InFlight {
    counters: Arc<Counters>,
    key: String,
}

impl InFlight {
    fn enter(counters: &Arc<Counters>, key: &str) -> Self {
        let n = counters.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        counters.max_in_flight.fetch_max(n, Ordering::SeqCst);
        counters.total.fetch_add(1, Ordering::SeqCst);
        {
            let mut map = counters.per_path.lock().unwrap();
            let e = map.entry(key.to_string()).or_insert((0, 0));
            e.0 += 1;
            e.1 = e.1.max(e.0);
        }
        Self {
            counters: counters.clone(),
            key: key.to_string(),
        }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.counters.in_flight.fetch_sub(1, Ordering::SeqCst);
        if let Some(e) = self.counters.per_path.lock().unwrap().get_mut(&self.key) {
            e.0 -= 1;
        }
    }
}

async fn serve(
    mut stream: TcpStream,
    handler: Handler,
    counters: Arc<Counters>,
    log: Arc<Mutex<Vec<Request>>>,
) -> std::io::Result<()> {
    let Some(request) = read_request(&mut stream).await? else {
        return Ok(());
    };
    log.lock().unwrap().push(request.clone());
    let _guard = InFlight::enter(&counters, request.path());
    let reply = handler(request).await;
    tokio::time::sleep(reply.delay).await;
    let total: usize = reply.chunks.iter().map(|(_, b)| b.len()).sum();
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    for (k, v) in &reply.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    match reply.length {
        Length::Exact => head.push_str(&format!("content-length: {total}\r\n")),
        Length::Wrong(n) => head.push_str(&format!("content-length: {n}\r\n")),
        Length::None => {}
    }
    head.push_str("connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    for (delay, bytes) in &reply.chunks {
        tokio::time::sleep(*delay).await;
        stream.write_all(bytes).await?;
        stream.flush().await?;
    }
    stream.shutdown().await
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 1 << 20 {
            return Ok(None);
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let start = lines.next().unwrap_or_default();
    let mut parts = start.split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);
    Ok(Some(Request {
        method,
        target,
        headers,
        body,
    }))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        301 => "Moved Permanently",
        302 => "Found",
        307 => "Temporary Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        432 => "Plan Limit Exceeded",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

/// Decode `%XX` and `+` in a query component.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
