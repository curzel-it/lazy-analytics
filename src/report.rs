use crate::store::Store;
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};

const DAY: i64 = 24 * 60 * 60 * 1000;
const TOP_VALUES: i64 = 12;
const DURATION_BUCKETS: [(&str, i64); 6] = [("<1m", 60), ("1–5m", 300), ("5–15m", 900), ("15–30m", 1800), ("30–60m", 3600), ("1h+", i64::MAX)];

/// The window a report covers: the last `days` local days up to `now`, the browser being `tz_minutes` east of UTC.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub days: i64,
    pub tz_minutes: i64,
    pub now: i64,
}

impl Window {
    fn offset(&self) -> i64 {
        self.tz_minutes * 60 * 1000
    }
    fn today(&self) -> i64 {
        (self.now + self.offset()).div_euclid(DAY)
    }
    fn from(&self) -> i64 {
        (self.today() - self.days + 1) * DAY - self.offset()
    }
}

/// `YYYY-MM-DD` of the `day`th day since 1970-01-01.
fn date(day: i64) -> String {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn counts(conn: &Connection, sql: &str, args: impl rusqlite::Params) -> rusqlite::Result<Map<String, Value>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(args, |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    rows.map(|row| row.map(|(k, v)| (k, json!(v)))).collect()
}

fn key_entry(params: &mut Map<String, Value>, name: String, key: String) -> &mut Map<String, Value> {
    let keys = params.entry(name).or_insert_with(|| json!({}));
    let Value::Object(keys) = keys else { unreachable!() };
    let entry = keys.entry(key).or_insert_with(|| json!({ "top": [] }));
    let Value::Object(entry) = entry else { unreachable!() };
    entry
}

/// The latest `limit` events, newest first, before `before` and named `name` when given.
pub fn recent(store: &Store, name: Option<&str>, before: i64, limit: i64) -> rusqlite::Result<Value> {
    let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
    let mut statement = conn.prepare(
        "SELECT id, at, name, user, session, install, source, payload FROM analytics_events
         WHERE at < ?1 AND (?2 IS NULL OR name = ?2) ORDER BY at DESC, id DESC LIMIT ?3",
    )?;
    let rows = statement.query_map(params![before, name, limit], |r| {
        let payload: Option<String> = r.get(7)?;
        Ok(json!({
            "id": r.get::<_, i64>(0)?,
            "at": r.get::<_, i64>(1)?,
            "name": r.get::<_, String>(2)?,
            "user": r.get::<_, Option<String>>(3)?,
            "session": r.get::<_, Option<String>>(4)?,
            "install": r.get::<_, Option<String>>(5)?,
            "source": r.get::<_, String>(6)?,
            "payload": payload.and_then(|p| serde_json::from_str::<Value>(&p).ok()),
        }))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map(Value::from)
}

/// Everything the dashboard shows for `window`.
pub fn report(store: &Store, window: Window) -> rusqlite::Result<Value> {
    let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
    let (from, offset) = (window.from(), window.offset());
    let duration = "(last_seen_at - started_at) / 1000";

    let (installs, sessions, play, average): (i64, i64, i64, f64) = conn.query_row(
        &format!("SELECT COUNT(DISTINCT install), COUNT(*), COALESCE(SUM({duration}), 0), COALESCE(AVG({duration}), 0) FROM analytics_sessions WHERE started_at >= ?"),
        [from],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let median: i64 = conn
        .query_row(
            &format!("SELECT {duration} AS d FROM analytics_sessions WHERE started_at >= ? ORDER BY d LIMIT 1 OFFSET ?"),
            params![from, sessions / 2],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let new_installs: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT install FROM analytics_sessions GROUP BY install HAVING MIN(started_at) >= ?)",
        [from],
        |r| r.get(0),
    )?;
    let users: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT user FROM analytics_events WHERE at >= ?1 AND user IS NOT NULL UNION SELECT user FROM analytics_sessions WHERE started_at >= ?1 AND user IS NOT NULL)",
        [from],
        |r| r.get(0),
    )?;
    let client = counts(&conn, "SELECT name, COUNT(*) FROM analytics_events WHERE at >= ? AND source = 'client' GROUP BY name ORDER BY 2 DESC", [from])?;
    let server = counts(&conn, "SELECT name, COUNT(*) FROM analytics_events WHERE at >= ? AND source = 'server' GROUP BY name ORDER BY 2 DESC", [from])?;

    let mut daily: Vec<Map<String, Value>> = (0..window.days)
        .map(|i| {
            let mut day = Map::new();
            day.insert("day".into(), json!(date(window.today() - window.days + 1 + i)));
            for key in ["installs", "sessions", "playSeconds"] {
                day.insert(key.into(), json!(0));
            }
            day.insert("client".into(), json!({}));
            day.insert("server".into(), json!({}));
            day
        })
        .collect();
    let first_day = window.today() - window.days + 1;
    let length = daily.len();
    let slot = |d: i64| usize::try_from(d - first_day).ok().filter(|&i| i < length);
    {
        let mut statement = conn.prepare(&format!(
            "SELECT (started_at + ?1) / {DAY} AS d, COUNT(DISTINCT install), COUNT(*), SUM({duration}) FROM analytics_sessions WHERE started_at >= ?2 GROUP BY d"
        ))?;
        let rows = statement.query_map(params![offset, from], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))?;
        for row in rows {
            let (d, installs, sessions, play) = row?;
            if let Some(i) = slot(d) {
                daily[i].insert("installs".into(), json!(installs));
                daily[i].insert("sessions".into(), json!(sessions));
                daily[i].insert("playSeconds".into(), json!(play));
            }
        }
        let mut statement = conn.prepare(&format!(
            "SELECT (at + ?1) / {DAY} AS d, source, name, COUNT(*) FROM analytics_events WHERE at >= ?2 GROUP BY d, source, name"
        ))?;
        let rows = statement.query_map(params![offset, from], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?;
        for row in rows {
            let (d, source, name, count) = row?;
            if let Some(Value::Object(by_name)) = slot(d).and_then(|i| daily[i].get_mut(&source)) {
                by_name.insert(name, json!(count));
            }
        }
    }

    let mut params = Map::new();
    {
        let mut statement = conn.prepare(
            "SELECT name, key, value, n, installs FROM (
               SELECT e.name, j.key, j.value, COUNT(*) AS n, COUNT(DISTINCT e.install) AS installs,
                      ROW_NUMBER() OVER (PARTITION BY e.name, j.key ORDER BY COUNT(*) DESC) AS rank
               FROM analytics_events e, json_each(e.payload) j
               WHERE e.at >= ? AND e.payload IS NOT NULL AND j.type IN ('text', 'true', 'false')
               GROUP BY e.name, j.key, j.value)
             WHERE rank <= ? ORDER BY name, key, n DESC",
        )?;
        let rows = statement.query_map(params![from, TOP_VALUES], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, rusqlite::types::Value>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
        })?;
        for row in rows {
            let (name, key, value, count, installs) = row?;
            let value = match value {
                rusqlite::types::Value::Text(t) => t,
                rusqlite::types::Value::Integer(i) => (i != 0).to_string(),
                other => format!("{other:?}"),
            };
            let entry = key_entry(&mut params, name, key);
            if let Some(Value::Array(list)) = entry.get_mut("top") {
                list.push(json!({ "value": value, "count": count, "installs": installs }));
            }
        }
        let mut statement = conn.prepare(
            "SELECT e.name, j.key, COUNT(*), AVG(j.value), MIN(j.value), MAX(j.value), SUM(j.value)
             FROM analytics_events e, json_each(e.payload) j
             WHERE e.at >= ? AND e.payload IS NOT NULL AND j.type IN ('integer', 'real')
             GROUP BY e.name, j.key ORDER BY e.name, j.key",
        )?;
        let rows = statement.query_map([from], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, f64>(3)?, r.get::<_, f64>(4)?, r.get::<_, f64>(5)?, r.get::<_, f64>(6)?))
        })?;
        for row in rows {
            let (name, key, count, avg, min, max, sum) = row?;
            key_entry(&mut params, name, key).insert("numbers".into(), json!({ "count": count, "avg": avg, "min": min, "max": max, "sum": sum }));
        }
    }

    let breakdown = |column: &str| -> rusqlite::Result<Value> {
        let mut statement = conn.prepare(&format!(
            "SELECT COALESCE({column}, '?') AS k, COUNT(*), COUNT(DISTINCT install) FROM analytics_sessions WHERE started_at >= ? GROUP BY k ORDER BY 2 DESC LIMIT 12"
        ))?;
        let rows = statement.query_map([from], |r| Ok(json!({ "key": r.get::<_, String>(0)?, "sessions": r.get::<_, i64>(1)?, "installs": r.get::<_, i64>(2)? })))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map(Value::from)
    };

    let mut durations = vec![];
    let mut floor = 0;
    for (bucket, ceiling) in DURATION_BUCKETS {
        let n: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM analytics_sessions WHERE started_at >= ? AND {duration} >= ? AND {duration} < ?"),
            params![from, floor, ceiling],
            |r| r.get(0),
        )?;
        durations.push(json!({ "bucket": bucket, "sessions": n }));
        floor = ceiling;
    }

    Ok(json!({
        "from": from,
        "to": window.now,
        "days": window.days,
        "totals": {
            "installs": installs,
            "users": users,
            "newInstalls": new_installs,
            "sessions": sessions,
            "playSeconds": play,
            "averageSessionSeconds": average.round() as i64,
            "medianSessionSeconds": median,
        },
        "client": client,
        "server": server,
        "daily": daily,
        "params": params,
        "platforms": breakdown("platform")?,
        "builds": breakdown("build")?,
        "durations": durations,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{parse, Source};

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(20_405), "2025-11-13");
    }

    #[test]
    fn reports_sessions_events_and_labels() {
        let store = crate::store::open(":memory:").unwrap();
        let now = 1_763_000_000_000;
        let open = parse(br#"{"install":"a","session":"s1","platform":"ios","events":[{"name":"session_start"},{"name":"minigame","payload":{"game":"fetch","score":12}}]}"#, Source::Client).unwrap();
        store.record(&open, now - 600_000).unwrap();
        let ping = parse(br#"{"install":"a","session":"s1","platform":"ios"}"#, Source::Client).unwrap();
        store.record(&ping, now - 300_000).unwrap();
        store.record(&parse(br#"{"events":[{"name":"signup","user":1,"payload":{"method":"email"}}]}"#, Source::Server).unwrap(), now).unwrap();
        let r = report(&store, Window { days: 7, tz_minutes: 60, now }).unwrap();
        assert_eq!(r["totals"]["installs"], 1);
        assert_eq!(r["totals"]["medianSessionSeconds"], 300);
        assert_eq!(r["client"]["minigame"], 1);
        assert_eq!(r["server"]["signup"], 1);
        assert_eq!(r["params"]["minigame"]["game"]["top"][0]["value"], "fetch");
        assert_eq!(r["params"]["minigame"]["score"]["numbers"]["max"], 12.0);
        assert_eq!(r["totals"]["users"], 1);
        let latest = recent(&store, Some("signup"), now + 1, 10).unwrap();
        assert_eq!(latest[0]["payload"]["method"], "email");
        assert_eq!(recent(&store, None, now + 1, 10).unwrap().as_array().unwrap().len(), 3);
        assert_eq!(r["platforms"][0]["key"], "ios");
        assert_eq!(r["daily"].as_array().unwrap().len(), 7);
        assert_eq!(r["daily"][6]["server"]["signup"], 1);
    }
}
