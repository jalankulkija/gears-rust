use toolkit_macros::domain_model;

/// A single host-matching rule. Target hosts are configuration rather than
/// compiled-in constants, so a deployment can point the proxy at different
/// hosts without a rebuild.
#[domain_model]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPattern {
    /// Matches only this exact host (case-insensitive).
    Exact(String),
    /// Matches this host or any subdomain of it (case-insensitive) — e.g.
    /// `Suffix("oaiusercontent.com")` matches `oaiusercontent.com` and
    /// `sdmntprcacentral.oaiusercontent.com`.
    Suffix(String),
}

impl HostPattern {
    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        match self {
            Self::Exact(exact) => host.eq_ignore_ascii_case(exact),
            Self::Suffix(suffix) => {
                let host = host.to_ascii_lowercase();
                let suffix = suffix.to_ascii_lowercase();
                host == suffix || host.ends_with(&format!(".{suffix}"))
            }
        }
    }
}

/// Ordered set of [`HostPattern`]s deciding which target hosts
/// `forward-proxy` runs its handler pipeline for at all. Hosts that don't
/// match are forwarded untouched (still TLS-terminated by hudsucker, since
/// that's required to even read the `Host` header, but no handler runs).
#[domain_model]
#[derive(Debug, Clone, Default)]
pub struct HostMatcher {
    patterns: Vec<HostPattern>,
}

impl HostMatcher {
    #[must_use]
    pub fn new(patterns: Vec<HostPattern>) -> Self {
        Self { patterns }
    }

    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        self.patterns.iter().any(|pattern| pattern.matches(host))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn exact_pattern_matches_only_that_host() {
        let pattern = HostPattern::Exact("chatgpt.com".to_owned());
        assert!(pattern.matches("chatgpt.com"));
        assert!(pattern.matches("CHATGPT.COM"), "should be case-insensitive");
        assert!(!pattern.matches("api.chatgpt.com"));
        assert!(!pattern.matches("notchatgpt.com"));
    }

    #[test]
    fn suffix_pattern_matches_base_host_and_subdomains() {
        let pattern = HostPattern::Suffix("oaiusercontent.com".to_owned());
        assert!(pattern.matches("oaiusercontent.com"));
        assert!(pattern.matches("sdmntprcacentral.oaiusercontent.com"));
        assert!(pattern.matches("SDMNTPRPOLANDCENTRAL.OAIUSERCONTENT.COM"));
        assert!(!pattern.matches("notoaiusercontent.com"));
        assert!(!pattern.matches("oaiusercontent.com.evil.com"));
    }

    #[test]
    fn host_matcher_matches_if_any_pattern_matches() {
        let matcher = HostMatcher::new(vec![
            HostPattern::Exact("chatgpt.com".to_owned()),
            HostPattern::Suffix("oaiusercontent.com".to_owned()),
        ]);
        assert!(matcher.matches("chatgpt.com"));
        assert!(matcher.matches("sdmntprcacentral.oaiusercontent.com"));
        assert!(!matcher.matches("example.com"));
    }

    #[test]
    fn empty_host_matcher_matches_nothing() {
        let matcher = HostMatcher::default();
        assert!(!matcher.matches("chatgpt.com"));
    }
}
