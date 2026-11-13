use crate::http::{get_status, Request};

/// Decides whether a request comes from an admin, the only one who may read the analytics. The host
/// supplies it: a closure in Rust, or [`Forward`] for a host in another language.
pub trait AdminCheck: Send + Sync {
    fn is_admin(&self, request: &Request) -> bool;
}

impl<F: Fn(&Request) -> bool + Send + Sync> AdminCheck for F {
    fn is_admin(&self, request: &Request) -> bool {
        self(request)
    }
}

/// Asks the host: GETs `url` with the request's `Authorization` and `Cookie` headers, and the caller is
/// an admin when the host answers 200.
pub struct Forward {
    pub url: String,
}

impl AdminCheck for Forward {
    fn is_admin(&self, request: &Request) -> bool {
        let headers: Vec<(&str, &str)> = [("Authorization", "authorization"), ("Cookie", "cookie")]
            .into_iter()
            .filter_map(|(name, key)| request.header(key).map(|v| (name, v)))
            .collect();
        !headers.is_empty() && get_status(&self.url, &headers) == Some(200)
    }
}
