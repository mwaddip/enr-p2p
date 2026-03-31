use crate::types::{Network, ProxyMode};
use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub listen: ListenConfig,
    pub outbound: OutboundConfig,
    pub identity: IdentityConfig,
}

#[derive(Debug, Deserialize)]
pub struct ProxyConfig {
    pub network: Network,
}

#[derive(Debug, Deserialize)]
pub struct ListenConfig {
    pub ipv6: Option<ListenerConfig>,
    pub ipv4: Option<ListenerConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ListenerConfig {
    pub address: SocketAddr,
    pub mode: ProxyMode,
    pub max_inbound: usize,
}

#[derive(Debug, Deserialize)]
pub struct OutboundConfig {
    pub min_peers: usize,
    pub max_peers: usize,
    pub seed_peers: Vec<SocketAddr>,
}

#[derive(Debug, Deserialize)]
pub struct IdentityConfig {
    pub agent_name: String,
    pub peer_name: String,
    pub protocol_version: String,
}

impl Config {
    /// Load config from a TOML file.
    ///
    /// # Contract
    /// - **Precondition**: `path` points to a readable TOML file.
    /// - **Postcondition**: Returns a valid `Config` with at least one listener
    ///   and at least one seed peer, or an error.
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;

        if config.listen.ipv4.is_none() && config.listen.ipv6.is_none() {
            return Err("At least one listener (ipv4 or ipv6) must be configured".into());
        }

        if config.outbound.seed_peers.is_empty() {
            return Err("At least one seed peer must be configured".into());
        }

        if config.outbound.min_peers > config.outbound.max_peers {
            return Err("min_peers must be <= max_peers".into());
        }

        Ok(config)
    }

    /// Parse the protocol version string into (major, minor, patch).
    ///
    /// # Contract
    /// - **Precondition**: `identity.protocol_version` is "X.Y.Z" format.
    /// - **Postcondition**: Returns three u8 values.
    pub fn version_bytes(&self) -> Result<(u8, u8, u8), Box<dyn std::error::Error>> {
        let parts: Vec<&str> = self.identity.protocol_version.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("Invalid version: {}", self.identity.protocol_version).into());
        }
        Ok((parts[0].parse()?, parts[1].parse()?, parts[2].parse()?))
    }
}
