use crate::admin::AdminCheck;
use crate::dashboard::DashboardConfig;
use crate::events::{parse, Source};
use crate::http::{read_request, write_response, Request, Response};
use crate::limiter::Limiter;
use crate::report::{recent, report, Window};
use crate::store::Store;
use serde_json::json;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_DAYS: i64 = 400;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The analytics server, its paths relative to wherever a proxy mounts it:
///
/// - `GET /` the dashboard page, which holds no data itself
/// - `POST /api/events` a batch of events: anonymous from anyone, naming users only from this machine
/// - `GET /api/report?days=30&tz=60` the dashboard's numbers, for admins only
/// - `GET /api/events?name=&before=&limit=100` the latest events with their payloads, for admins only
pub struct Server {
    store: Store,
    admin: Box<dyn AdminCheck>,
    pub dashboard: DashboardConfig,
    /// Days of history kept; older events and sessions are deleted once a day. `None` keeps them all.
    pub retention_days: Option<i64>,
    limiter: Limiter,
}

impl Server {
    pub fn new(store: Store, admin: impl AdminCheck + 'static) -> Server {
        Server {
            store,
            admin: Box::new(admin),
            dashboard: DashboardConfig { title: "Analytics".into(), token_key: None },
            retention_days: None,
            limiter: Limiter::new(120),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Answers one request.
    pub fn handle(&self, request: &Request) -> Response {
        let cors = |r: Response| {
            r.with("Access-Control-Allow-Origin", "*")
                .with("Access-Control-Allow-Headers", "Content-Type")
                .with("Access-Control-Allow-Methods", "POST, OPTIONS")
                .with("Access-Control-Max-Age", "86400")
        };
        match (request.method.as_str(), request.path.as_str()) {
            ("GET" | "HEAD", "/" | "/index.html") => Response::html(self.dashboard.page()),
            ("OPTIONS", "/api/events") => cors(Response::json(204, &json!(null))).empty(),
            ("POST", "/api/events") => {
                let local = request.is_local();
                if !local && !self.limiter.allow(&request.client_ip(), now()) {
                    return cors(Response::json(429, &json!({ "error": "slowDown" })));
                }
                let source = if local { Source::Server } else { Source::Client };
                let Some(batch) = parse(&request.body, source) else {
                    return cors(Response::json(400, &json!({ "error": "badBatch" })));
                };
                match self.store.record(&batch, now()) {
                    Ok(()) => cors(Response::json(200, &json!({}))),
                    Err(error) => {
                        eprintln!("[analytics] {error}");
                        cors(Response::json(500, &json!({ "error": "serverError" })))
                    }
                }
            }
            ("GET", "/api/report") => {
                if !self.admin.is_admin(request) {
                    return Response::json(403, &json!({ "error": "notAdmin" }));
                }
                let number = |key: &str, fallback: i64| request.query.get(key).and_then(|v| v.parse().ok()).unwrap_or(fallback);
                let window = Window { days: number("days", 30).clamp(1, MAX_DAYS), tz_minutes: number("tz", 0).clamp(-14 * 60, 14 * 60), now: now() };
                match report(&self.store, window) {
                    Ok(body) => Response::json(200, &body),
                    Err(error) => {
                        eprintln!("[analytics] {error}");
                        Response::json(500, &json!({ "error": "serverError" }))
                    }
                }
            }
            ("GET", "/api/events") => {
                if !self.admin.is_admin(request) {
                    return Response::json(403, &json!({ "error": "notAdmin" }));
                }
                let name = request.query.get("name").map(String::as_str).filter(|n| !n.is_empty());
                let before = request.query.get("before").and_then(|v| v.parse().ok()).unwrap_or(i64::MAX);
                let limit = request.query.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100).clamp(1, 500);
                match recent(&self.store, name, before, limit) {
                    Ok(body) => Response::json(200, &body),
                    Err(error) => {
                        eprintln!("[analytics] {error}");
                        Response::json(500, &json!({ "error": "serverError" }))
                    }
                }
            }
            (_, "/" | "/api/events" | "/api/report") => Response::json(405, &json!({ "error": "methodNotAllowed" })),
            _ => Response::json(404, &json!({ "error": "notFound" })),
        }
    }

    /// Serves on `address` until the process ends, one thread per connection.
    pub fn listen(self, address: &str) -> std::io::Result<()> {
        let listener = TcpListener::bind(address)?;
        let server = Arc::new(self);
        if let Some(days) = server.retention_days {
            let pruner = Arc::clone(&server);
            std::thread::spawn(move || loop {
                if let Err(error) = pruner.store.prune(now() - days * 24 * 60 * 60 * 1000) {
                    eprintln!("[analytics] prune: {error}");
                }
                std::thread::sleep(std::time::Duration::from_secs(24 * 60 * 60));
            });
        }
        for stream in listener.incoming().flatten() {
            let server = Arc::clone(&server);
            std::thread::spawn(move || {
                let response = match read_request(&stream) {
                    Ok(request) => {
                        let mut response = server.handle(&request);
                        if request.method == "HEAD" {
                            response.body.clear();
                        }
                        response
                    }
                    Err(status) => Response::json(status, &json!({ "error": "badRequest" })),
                };
                write_response(&stream, &response);
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn request(method: &str, path: &str, body: &str, peer: [u8; 4]) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            body: body.as_bytes().to_vec(),
            peer: Some(IpAddr::V4(Ipv4Addr::from(peer))),
            ..Default::default()
        }
    }

    #[test]
    fn reports_only_to_admins_and_trusts_only_local_users() {
        let server = Server::new(crate::store::open(":memory:").unwrap(), |r: &Request| r.header("authorization") == Some("Bearer ok"));
        let remote = request("POST", "/api/events", r#"{"install":"i","session":"s","events":[{"name":"race","payload":{"league":"cup"}}]}"#, [8, 8, 8, 8]);
        assert_eq!(server.handle(&remote).status, 200);
        let local = request("POST", "/api/events", r#"{"events":[{"name":"login","user":"3"}]}"#, [127, 0, 0, 1]);
        assert_eq!(server.handle(&local).status, 200);
        let mut ask = request("GET", "/api/report", "", [8, 8, 8, 8]);
        assert_eq!(server.handle(&ask).status, 403);
        ask.headers.insert("authorization".into(), "Bearer ok".into());
        let answer: serde_json::Value = serde_json::from_slice(&server.handle(&ask).body).unwrap();
        assert_eq!(answer["client"]["race"], 1);
        assert_eq!(answer["server"]["login"], 1);
        let mut latest = request("GET", "/api/events", "", [8, 8, 8, 8]);
        assert_eq!(server.handle(&latest).status, 403);
        latest.headers.insert("authorization".into(), "Bearer ok".into());
        let events: serde_json::Value = serde_json::from_slice(&server.handle(&latest).body).unwrap();
        assert_eq!(events.as_array().unwrap().len(), 2);
        assert!(server.handle(&request("GET", "/", "", [8, 8, 8, 8])).body.len() > 1000);
    }
}
