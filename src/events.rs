use serde_json::Value;

/// The most events one batch may carry.
pub const MAX_EVENTS: usize = 200;
/// How far back an event may say it happened, for a device that queued it offline.
const LONGEST_DELAY: i64 = 24 * 60 * 60 * 1000;
const MAX_PAYLOAD: usize = 4096;

/// Who sent a batch: a player's device, whose word on who the user is is only a claim, or the host's
/// own server, whose word is trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Client,
    Server,
}

#[derive(Debug, PartialEq)]
pub struct Event {
    pub name: String,
    /// Who it was about, when it differs from the batch's user.
    pub user: Option<String>,
    /// Any JSON object of up to 4 kB: the event's parameters.
    pub payload: Option<Value>,
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
    pub user: Option<String>,
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
/// and so is a payload that is not an object or is too big.
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
                Some(Event {
                    name: name(e.get("name"))?,
                    user: short(e.get("user"), 80),
                    payload: e.get("payload").filter(|p| p.is_object() && p.to_string().len() <= MAX_PAYLOAD).cloned(),
                    ago: e.get("ago").and_then(Value::as_i64).unwrap_or(0).clamp(0, LONGEST_DELAY),
                })
            })
            .collect(),
    };
    let batch = Batch {
        source,
        install: short(object.get("install"), 64),
        session: short(object.get("session"), 64),
        user: short(object.get("user"), 80),
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
    fn clients_need_a_session_and_bad_events_are_dropped() {
        assert!(parse(br#"{"events":[]}"#, Source::Client).is_none());
        let batch = parse(br#"{"install":"i","session":"s","user":"7","events":[{"name":"race","payload":{"league":"open"}},{"name":"bad name"},{"name":"x","payload":[1]}]}"#, Source::Client).unwrap();
        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.user.as_deref(), Some("7"));
        assert_eq!(batch.events[0].payload.as_ref().unwrap()["league"], "open");
        assert_eq!(batch.events[1].payload, None);
    }

    #[test]
    fn servers_name_users_and_delays_are_clamped() {
        let batch = parse(br#"{"events":[{"name":"login","user":7,"ago":999999999999}]}"#, Source::Server).unwrap();
        assert_eq!(batch.events[0].user.as_deref(), Some("7"));
        assert_eq!(batch.events[0].ago, LONGEST_DELAY);
    }
}
