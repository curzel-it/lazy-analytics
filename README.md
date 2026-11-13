# lazy-analytics

Small self-hosted product analytics for an app that already has a SQLite database. It keeps devices' sessions and the events you report in two tables of that database, and serves an admin-only dashboard as a single HTML page. It has no client SDK, cookies or fingerprinting, and it never sees your users' personal data.

## What it records

- **Sessions**: a random install id and session id from the device, its platform and build. A batch with no events is a heartbeat. A session lasts from its first batch to its last, capped at 6 hours.
- **Events**: a `name` (`[A-Za-z0-9_.-]`, up to 40 characters), an optional `label` (which minigame, which level...) and optional `props`, a small JSON object.
- **Server events**: batches that come straight from the same machine, not through a proxy, are trusted and may name a `user`. Use them for signups and logins.

The dashboard shows active and new installs, sessions, median and average session length, play time, a daily chart for every event name, the top labels of each event, and platforms and builds. Ranges run from 7 days to a year, in the viewer's time zone.

## Setup

```sh
lazy-analytics \
  --db sqlite:///var/lib/myapp/app.db \
  --admin-url http://127.0.0.1:8080/api/admin \
  --listen 127.0.0.1:8095 \
  --title "My app analytics" \
  --token-key myapp-session \
  --retention-days 400
```

Every flag can also come from the environment (`LAZY_ANALYTICS_DB`, `LAZY_ANALYTICS_ADMIN_URL`, ...).

- `--db` is an existing database or a new one. The service only creates and uses `analytics_sessions` and `analytics_events`. It runs in WAL mode with a busy timeout, so give your own connection a busy timeout too.
- `--admin-url` is how your app says who is an admin. The service forwards the viewer's `Authorization` and `Cookie` headers to it, and the viewer is an admin when the answer is `200`.
- `--token-key` is the `localStorage` key where your own pages keep the session token, as a bare token or as JSON with a `token` field. With it, an admin who is logged in on the same origin opens the dashboard without logging in again. Admins can also paste a token, or open `/#token=...`.

Mount it behind a proxy that sets `X-Real-IP`, for example nginx:

```nginx
location /analytics/ {
    proxy_pass http://127.0.0.1:8095/;
    proxy_set_header X-Real-IP $remote_addr;
}
```

## API

- `GET /`: the dashboard page. It contains no data.
- `POST /api/events`: anyone may post. Accepts `text/plain` so browsers skip the CORS preflight. Rate limited to 120 requests a minute per IP.

  ```json
  {"install": "…", "session": "…", "platform": "ios", "build": "42",
   "events": [{"name": "level_start", "label": "forest", "ago": 1200, "props": {"retry": true}}]}
  ```

  `ago` is how many milliseconds before sending the event happened, up to a day, for events queued while offline. A server batch needs only `events`, and each event may carry a `user`.
- `GET /api/report?days=30&tz=60`: the dashboard's numbers, for admins only. `tz` is the viewer's offset from UTC in minutes, east positive.

## As a library

```rust
let store = lazy_analytics::open("sqlite:///var/lib/myapp/app.db")?;
let server = lazy_analytics::Server::new(store, |request: &lazy_analytics::Request| {
    my_app::is_admin(request.header("authorization"))
});
server.listen("127.0.0.1:8095")?;
```

`Server::handle(&Request) -> Response` answers one request, so you can put it inside your own HTTP stack. `Store::record` and `report` are public too.

## Building

```sh
cargo build --release
# a static Linux binary from a Mac
cargo zigbuild --release --target x86_64-unknown-linux-musl
```
