//! Recognizes hosts this plugin cares about. Deliberately separate from
//! (and simpler than) `forward-proxy`'s own `domain::host_matcher` — that
//! one lives in the core gear's implementation crate, which this plugin
//! must not depend on (plugins depend on `forward-proxy-sdk` only). v1 has
//! no registry-level per-plugin host pre-filter (see
//! `forward_proxy_sdk::plugin_api` module docs), so every registered
//! handler sees every request that made it past the core's own
//! `HostMatcher` — this function is this plugin's own cheap self-filter.

const EXACT_HOSTS: &[&str] = &["chat.openai.com", "chatgpt.com", "api.openai.com"];
const SUFFIX_HOSTS: &[&str] = &["oaiusercontent.com"];
const CHATGPT_WEB_HOSTS: &[&str] = &["chat.openai.com", "chatgpt.com"];

#[must_use]
pub fn is_openai_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    EXACT_HOSTS.iter().any(|h| host == *h)
        || SUFFIX_HOSTS
            .iter()
            .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// Whether `host` is the `ChatGPT` web app itself (as opposed to the public
/// `api.openai.com` API) — it talks its own undocumented
/// `/backend-api/conversation` streaming protocol, not the public Chat
/// Completions SSE format, so a block response needs a different shape.
/// See `Service::block_outcome`'s docs.
#[must_use]
pub fn is_chatgpt_web_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    CHATGPT_WEB_HOSTS.iter().any(|h| host == *h)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn matches_known_exact_hosts() {
        assert!(is_openai_host("chatgpt.com"));
        assert!(is_openai_host("api.openai.com"));
        assert!(is_openai_host("CHAT.OPENAI.COM"));
    }

    #[test]
    fn matches_oaiusercontent_subdomains() {
        assert!(is_openai_host("sdmntprcacentral.oaiusercontent.com"));
        assert!(is_openai_host("oaiusercontent.com"));
    }

    #[test]
    fn rejects_unrelated_hosts() {
        assert!(!is_openai_host("example.com"));
        assert!(!is_openai_host("notoaiusercontent.com"));
    }

    #[test]
    fn chatgpt_web_host_matches_only_the_web_app() {
        assert!(is_chatgpt_web_host("chatgpt.com"));
        assert!(is_chatgpt_web_host("chat.openai.com"));
        assert!(!is_chatgpt_web_host("api.openai.com"));
    }
}
