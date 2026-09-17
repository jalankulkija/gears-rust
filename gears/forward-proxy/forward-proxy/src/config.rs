use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Configuration for the `forward-proxy` gear.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardProxyConfig {
    /// Address the proxy listens on — what browsers/OS proxy settings point at.
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    /// Where the persisted root CA certificate (PEM) is read from/written to.
    #[serde(default = "default_ca_cert_path")]
    pub ca_cert_path: PathBuf,
    /// Where the persisted root CA private key (PEM) is read from/written to.
    #[serde(default = "default_ca_key_path")]
    pub ca_key_path: PathBuf,
    /// Exact hostnames to run the handler pipeline for (see
    /// `domain::host_matcher::HostPattern::Exact`).
    #[serde(default)]
    pub intercept_hosts: Vec<String>,
    /// Hostname suffixes (this host or any subdomain) to run the handler
    /// pipeline for (see `domain::host_matcher::HostPattern::Suffix`).
    #[serde(default)]
    pub intercept_host_suffixes: Vec<String>,
    /// Per-handler call timeout, in milliseconds. A handler that doesn't
    /// respond within this window is treated as an error (fail-open — see
    /// `domain::pipeline::Pipeline`).
    #[serde(default = "default_handler_timeout_ms")]
    pub handler_timeout_ms: u64,
}

impl Default for ForwardProxyConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            ca_cert_path: default_ca_cert_path(),
            ca_key_path: default_ca_key_path(),
            intercept_hosts: Vec::new(),
            intercept_host_suffixes: Vec::new(),
            handler_timeout_ms: default_handler_timeout_ms(),
        }
    }
}

fn default_listen_addr() -> String {
    "127.0.0.1:8080".to_owned()
}

fn default_ca_cert_path() -> PathBuf {
    PathBuf::from("./forward-proxy-ca.cert.pem")
}

fn default_ca_key_path() -> PathBuf {
    PathBuf::from("./forward-proxy-ca.key.pem")
}

fn default_handler_timeout_ms() -> u64 {
    5_000
}
