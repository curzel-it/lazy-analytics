use crate::events::{Batch, Source};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS analytics_sessions (
  id TEXT PRIMARY KEY,
  install TEXT NOT NULL,
  platform TEXT,
  build TEXT,
  started_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS analytics_sessions_started ON analytics_sessions(started_at);
CREATE INDEX IF NOT EXISTS analytics_sessions_install ON analytics_sessions(install, started_at);
CREATE TABLE IF NOT EXISTS analytics_events (
  id INTEGER PRIMARY KEY,
  at INTEGER NOT NULL,
  name TEXT NOT NULL,
  label TEXT,
  source TEXT NOT NULL,
  session TEXT,
  install TEXT,
  user TEXT,
  props TEXT
);
CREATE INDEX IF NOT EXISTS analytics_events_at ON analytics_events(at);
CREATE INDEX IF NOT EXISTS analytics_events_name ON analytics_events(name, at);
";

/// A session never stretches past this, so a device left open on a table does not count for days.
const LONGEST_SESSION: i64 = 6 * 60 * 60 * 1000;

/// The analytics tables, kept beside the host's own in its database.
pub struct Store {
    pub(crate) conn: Mutex<Connection>,
}

/// Opens `url` (`sqlite:///abs/path.db`, `sqlite://rel.db`, a bare path or `:memory:`) and makes the
/// `analytics_*` tables when they are missing. Nothing else in the database is touched.
pub fn open(url: &str) -> rusqlite::Result<Store> {
    let path = url.strip_prefix("sqlite://").unwrap_or(url);
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(SCHEMA)?;
    Ok(Store { conn: Mutex::new(conn) })
}

impl Store {
    /// Records `batch` as received at `now`: touches its session and inserts its events.
    pub fn record(&self, batch: &Batch, now: i64) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction()?;
        if let (Some(session), Some(install)) = (&batch.session, &batch.install) {
            let started: Option<i64> = tx
                .query_row("SELECT started_at FROM analytics_sessions WHERE id = ?", [session], |r| r.get(0))
                .optional()?;
            match started {
                None => {
                    tx.execute(
                        "INSERT INTO analytics_sessions (id, install, platform, build, started_at, last_seen_at) VALUES (?, ?, ?, ?, ?, ?)",
                        params![session, install, batch.platform, batch.build, now, now],
                    )?;
                }
                Some(started) => {
                    tx.execute(
                        "UPDATE analytics_sessions SET last_seen_at = MAX(last_seen_at, ?) WHERE id = ?",
                        params![now.min(started + LONGEST_SESSION), session],
                    )?;
                }
            }
        }
        let source = match batch.source {
            Source::Client => "client",
            Source::Server => "server",
        };
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO analytics_events (at, name, label, source, session, install, user, props) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            for event in &batch.events {
                insert.execute(params![
                    now - event.ago,
                    event.name,
                    event.label,
                    source,
                    batch.session,
                    batch.install,
                    event.user,
                    event.props.as_ref().map(|p| p.to_string()),
                ])?;
            }
        }
        tx.commit()
    }

    /// Deletes events and sessions older than `before`.
    pub fn prune(&self, before: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("DELETE FROM analytics_events WHERE at < ?", [before])?;
        conn.execute("DELETE FROM analytics_sessions WHERE last_seen_at < ?", [before])?;
        Ok(())
    }
}
