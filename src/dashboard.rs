const PAGE: &str = include_str!("dashboard.html");

/// What the dashboard page is told: its title, and the `localStorage` key where the host's own pages
/// keep the admin's session token (a bare token, or JSON with a `token` field), so an admin already
/// logged in on the same origin needs no second login.
#[derive(Clone, Debug, Default)]
pub struct DashboardConfig {
    pub title: String,
    pub token_key: Option<String>,
}

impl DashboardConfig {
    pub fn page(&self) -> String {
        let config = serde_json::json!({ "title": self.title, "tokenKey": self.token_key });
        let safe = config.to_string().replace('<', "\\u003c");
        PAGE.replace("/*CONFIG*/{}", &safe).replace("<title>Analytics</title>", &format!("<title>{}</title>", escape(&self.title)))
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
