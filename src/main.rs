use lazy_analytics::{open, DashboardConfig, Forward, Server};

const USAGE: &str = "lazy-analytics --db <sqlite:///path.db> --admin-url <http://host/check> [--listen 127.0.0.1:8095] [--title Analytics] [--token-key <localStorage key>] [--retention-days <n>]

Keeps analytics_* tables in the database and serves the dashboard at /, events at POST /api/events and
the admin-only report at GET /api/report. A caller is an admin when --admin-url answers 200 to their
Authorization and Cookie headers.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        let env = format!("LAZY_ANALYTICS_{}", flag.trim_start_matches("--").replace('-', "_").to_uppercase());
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned()).or_else(|| std::env::var(env).ok())
    };
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    let (Some(db), Some(admin_url)) = (value("--db"), value("--admin-url")) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let store = open(&db).unwrap_or_else(|error| {
        eprintln!("[analytics] cannot open {db}: {error}");
        std::process::exit(1);
    });
    let mut server = Server::new(store, Forward { url: admin_url });
    server.dashboard = DashboardConfig { title: value("--title").unwrap_or_else(|| "Analytics".into()), token_key: value("--token-key") };
    server.retention_days = value("--retention-days").and_then(|d| d.parse().ok());
    let listen = value("--listen").unwrap_or_else(|| "127.0.0.1:8095".into());
    eprintln!("[analytics] http://{listen}/");
    if let Err(error) = server.listen(&listen) {
        eprintln!("[analytics] {error}");
        std::process::exit(1);
    }
}
