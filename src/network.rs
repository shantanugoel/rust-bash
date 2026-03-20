use std::collections::HashSet;
use std::time::Duration;

/// Policy controlling network access for commands like `curl`.
///
/// Disabled by default — scripts have no network access unless the embedder
/// explicitly enables it and configures an allow-list.
#[derive(Clone, Debug)]
pub struct NetworkPolicy {
    pub enabled: bool,
    pub allowed_url_prefixes: Vec<String>,
    pub allowed_methods: HashSet<String>,
    pub max_redirects: usize,
    pub max_response_size: usize,
    pub timeout: Duration,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_url_prefixes: Vec::new(),
            allowed_methods: HashSet::from(["GET".to_string(), "POST".to_string()]),
            max_redirects: 5,
            max_response_size: 10 * 1024 * 1024, // 10 MB
            timeout: Duration::from_secs(30),
        }
    }
}

impl NetworkPolicy {
    /// Validate that `url` matches at least one entry in `allowed_url_prefixes`.
    ///
    /// The raw URL is first parsed and re-serialized via `url::Url` to
    /// normalize it (resolve default ports, percent-encoding, etc.), and then
    /// each allowed prefix is checked with a simple `starts_with`.
    /// Prefixes are also normalized via `url::Url` when possible to prevent
    /// subdomain confusion attacks (e.g. a prefix of `"https://api.example.com"`
    /// without a trailing slash would otherwise match `"https://api.example.com.evil.com/"`).
    pub fn validate_url(&self, url: &str) -> Result<(), String> {
        let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL '{url}': {e}"))?;
        let normalized = parsed.as_str();

        for prefix in &self.allowed_url_prefixes {
            let norm_prefix = url::Url::parse(prefix)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| prefix.clone());
            if normalized.starts_with(&norm_prefix) {
                return Ok(());
            }
        }

        Err(format!("URL not allowed by network policy: {normalized}"))
    }

    /// Validate that `method` is in the set of allowed HTTP methods.
    pub fn validate_method(&self, method: &str) -> Result<(), String> {
        let upper = method.to_uppercase();
        if self.allowed_methods.contains(&upper) {
            Ok(())
        } else {
            Err(format!(
                "HTTP method not allowed by network policy: {upper}"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        let policy = NetworkPolicy::default();
        assert!(!policy.enabled);
    }

    #[test]
    fn default_allows_get_and_post() {
        let policy = NetworkPolicy::default();
        assert!(policy.allowed_methods.contains("GET"));
        assert!(policy.allowed_methods.contains("POST"));
        assert!(!policy.allowed_methods.contains("DELETE"));
    }

    #[test]
    fn validate_url_matches_prefix() {
        let policy = NetworkPolicy {
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            ..Default::default()
        };
        assert!(
            policy
                .validate_url("https://api.example.com/v1/data")
                .is_ok()
        );
        assert!(
            policy
                .validate_url("https://api.example.com/users?id=1")
                .is_ok()
        );
    }

    #[test]
    fn validate_url_rejects_different_domain() {
        let policy = NetworkPolicy {
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            ..Default::default()
        };
        assert!(
            policy
                .validate_url("https://api.example.com.evil.org/")
                .is_err()
        );
    }

    #[test]
    fn validate_url_rejects_different_scheme() {
        let policy = NetworkPolicy {
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            ..Default::default()
        };
        assert!(policy.validate_url("http://api.example.com/").is_err());
    }

    #[test]
    fn validate_url_rejects_subdomain_without_trailing_slash() {
        let policy = NetworkPolicy {
            allowed_url_prefixes: vec!["https://api.example.com".to_string()],
            ..Default::default()
        };
        // Must NOT match evil subdomain even without trailing slash in prefix
        assert!(
            policy
                .validate_url("https://api.example.com.evil.com/")
                .is_err()
        );
        // But the intended domain should still work
        assert!(
            policy
                .validate_url("https://api.example.com/v1/data")
                .is_ok()
        );
    }

    #[test]
    fn validate_url_rejects_userinfo_attack() {
        let policy = NetworkPolicy {
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            ..Default::default()
        };
        // url::Url normalizes this so the prefix check catches it
        assert!(
            policy
                .validate_url("https://api.example.com@evil.com/")
                .is_err()
        );
    }

    #[test]
    fn validate_url_no_prefixes_rejects_all() {
        let policy = NetworkPolicy::default();
        assert!(policy.validate_url("https://example.com/").is_err());
    }

    #[test]
    fn validate_url_invalid_url() {
        let policy = NetworkPolicy::default();
        assert!(policy.validate_url("not a url").is_err());
    }

    #[test]
    fn validate_method_allowed() {
        let policy = NetworkPolicy::default();
        assert!(policy.validate_method("GET").is_ok());
        assert!(policy.validate_method("get").is_ok());
        assert!(policy.validate_method("POST").is_ok());
    }

    #[test]
    fn validate_method_rejected() {
        let policy = NetworkPolicy::default();
        assert!(policy.validate_method("DELETE").is_err());
        assert!(policy.validate_method("PUT").is_err());
    }
}
