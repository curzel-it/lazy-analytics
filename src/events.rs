use serde_json::Value;

/// The most events one batch may carry.
pub const MAX_EVENTS: usize = 200;
/// How far back an event may say it happened, for a device that queued it offline.
const LONGEST_DELAY: i64 = 24 * 60 * 60 * 1000;
const MAX_PROPS: usize = 2048;

/// Who sent a batch: a player's device, or the host's own server, the only one that may name a user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Client,
    Server,
}

#[derive(Debug, PartialEq)]
pub struct Event {
    pub name: String,
    pub label: Option<String>,
    pub user: Option<String>,
    pub props: Option<Value>,
    /// How many milliseconds before the batch arrived it happened.
    pub ago: i64,
}

/// What one request reports: a device's session and the events since its last batch. A batch with no
/// events is a heartbeat that keeps the session alive.
#[derive(Debug, PartialEq)]
pub struct Batch {
    pub source: Source,
    pub install: Option<String>,
    pub session: Option<String>,
    pub platform: Option<String>,
    pub build: Option<String>,
    pub events: Vec<Event>,
}

fn short(value: Option<&Value>, max: usize) -> Option<String> {
    let text = match value? {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then(|| text.chars().take(max).collect())
}

fn name(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    let valid = !text.is_empty() && text.len() <= 40 && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-');
    valid.then(|| text.to_string())
}

/// Reads a batch out of a request body; `None` when it is not one. Events with a bad name are dropped,
/// and a client may not name a user.
pub fn parse(body: &[u8], source: Source) -> Option<Batch> {
    let json: Value = serde_json::from_slice(body).ok()?;
    let object = json.as_object()?;
    let events = match object.get("events") {
        None => vec![],
        Some(list) => list
            .as_array()?
            .iter()
            .take(MAX_EVENTS)
            .filter_map(|e| {
                let props = e.get("props").filter(|p| p.is_object() && p.to_string().len() <= MAX_PROPS).cloned();
                Some(Event {
                    name: name(e.get("name"))?,
                    label: short(e.get("label"), 80),
                    user: if source == Source::Server { short(e.get("user"), 80) } else { None },
                    props,
                    ago: e.get("ago").and_then(Value::as_i64).unwrap_or(0).clamp(0, LONGEST_DELAY),
                })
            })
            .collect(),
    };
    let batch = Batch {
        source,
        install: short(object.get("install"), 64),
        session: short(object.get("session"), 64),
        platform: short(object.get("platform"), 24),
        build: short(object.get("build"), 24),
        events,
    };
    (source == Source::Server || (batch.install.is_some() && batch.session.is_some())).then_some(batch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients_need_a_session_and_cannot_name_users() {
        assert!(parse(br#"{"events":[]}"#, Source::Client).is_none());
        let batch = parse(br#"{"install":"i","session":"s","events":[{"name":"race","user":"7"},{"name":"bad name"}]}"#, Source::Client).unwrap();
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].user, None);
    }

    #[test]
    fn servers_name_users_and_delays_are_clamped() {
        let batch = parse(br#"{"events":[{"name":"login","user":7,"ago":999999999999}]}"#, Source::Server).unwrap();
        assert_eq!(batch.events[0].user.as_deref(), Some("7"));
        assert_eq!(batch.events[0].ago, LONGEST_DELAY);
    }
}
