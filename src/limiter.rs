use std::collections::HashMap;
use std::sync::Mutex;

/// Allows each key `per_minute` requests in a rolling minute window.
pub struct Limiter {
    per_minute: u32,
    seen: Mutex<HashMap<String, (i64, u32)>>,
}

impl Limiter {
    pub fn new(per_minute: u32) -> Limiter {
        Limiter { per_minute, seen: Mutex::new(HashMap::new()) }
    }

    pub fn allow(&self, key: &str, now: i64) -> bool {
        let minute = now / 60_000;
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() > 50_000 {
            seen.retain(|_, (m, _)| *m == minute);
        }
        let entry = seen.entry(key.to_string()).or_insert((minute, 0));
        if entry.0 != minute {
            *entry = (minute, 0);
        }
        entry.1 += 1;
        entry.1 <= self.per_minute
    }
}
