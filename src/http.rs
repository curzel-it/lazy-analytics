use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

const MAX_HEAD: usize = 16 * 1024;
/// The largest request body accepted.
pub const MAX_BODY: usize = 64 * 1024;

/// One HTTP/1.1 request, its header names lowercased.
#[derive(Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub peer: Option<IpAddr>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
    /// The caller's address: the proxy's `X-Real-IP` when there is one, else the socket's peer.
    pub fn client_ip(&self) -> String {
        self.header("x-real-ip").map(str::to_string).or_else(|| self.peer.map(|p| p.to_string())).unwrap_or_default()
    }
    /// True for a call straight from this machine, not relayed by a proxy.
    pub fn is_local(&self) -> bool {
        self.peer.is_some_and(|p| p.is_loopback()) && self.header("x-real-ip").is_none() && self.header("x-forwarded-for").is_none()
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, body: &serde_json::Value) -> Response {
        Response { status, content_type: "application/json; charset=utf-8", headers: vec![], body: body.to_string().into_bytes() }
    }
    pub fn html(body: String) -> Response {
        Response { status: 200, content_type: "text/html; charset=utf-8", headers: vec![], body: body.into_bytes() }
    }
    pub fn empty(mut self) -> Response {
        self.body.clear();
        self
    }
    pub fn with(mut self, name: &'static str, value: &str) -> Response {
        self.headers.push((name, value.to_string()));
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        _ => "Error",
    }
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&text[i + 1..i + 3], 16) {
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

/// Reads one request off `stream`; `Err` carries the status to answer a bad one with.
pub fn read_request(stream: &TcpStream) -> Result<Request, u16> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut reader = BufReader::new(stream.take((MAX_HEAD + MAX_BODY) as u64));
    let mut line = String::new();
    let mut head = 0;
    reader.read_line(&mut line).map_err(|_| 400u16)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().ok_or(400u16)?.to_string(), parts.next().ok_or(400u16)?.to_string());
    let mut headers = HashMap::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).map_err(|_| 400u16)?;
        head += n;
        if head > MAX_HEAD {
            return Err(413);
        }
        let trimmed = line.trim_end();
        if n == 0 || trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
    if length > MAX_BODY {
        return Err(413);
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|_| 400u16)?;
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let query = query.split('&').filter_map(|pair| pair.split_once('=')).map(|(k, v)| (decode(k), decode(v))).collect();
    Ok(Request { method, path: decode(path), query, headers, body, peer: stream.peer_addr().ok().map(|a| a.ip()) })
}

pub fn write_response(mut stream: &TcpStream, response: &Response) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        response.status,
        reason(response.status),
        response.content_type,
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&response.body));
}

/// GETs a plain `http://host:port/path` URL with `headers`, returning the status, or `None` when it
/// could not be reached.
pub fn get_status(url: &str, headers: &[(&str, &str)]) -> Option<u16> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let address = if host.contains(':') { host.to_string() } else { format!("{host}:80") };
    let socket = address.to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&socket, Duration::from_secs(3)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in headers {
        if !value.contains(['\r', '\n']) {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut status = String::new();
    BufReader::new(stream).read_line(&mut status).ok()?;
    status.split_whitespace().nth(1)?.parse().ok()
}
