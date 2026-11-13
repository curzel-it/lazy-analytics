//! Lazy analytics: a device's sessions and the events a product reports, kept in `analytics_*` tables
//! inside the host's own SQLite database, with an admin-only dashboard in one HTML page.
//!
//! ```no_run
//! let store = lazy_analytics::open("sqlite:///var/lib/app/app.db").unwrap();
//! let server = lazy_analytics::Server::new(store, |r: &lazy_analytics::Request| r.header("authorization") == Some("Bearer secret"));
//! server.listen("127.0.0.1:8095").unwrap();
//! ```

mod admin;
mod dashboard;
mod events;
mod http;
mod limiter;
mod report;
mod server;
mod store;

pub use admin::{AdminCheck, Forward};
pub use dashboard::DashboardConfig;
pub use events::{parse, Batch, Event, Source};
pub use http::{Request, Response};
pub use report::{report, Window};
pub use server::Server;
pub use store::{open, Store};
