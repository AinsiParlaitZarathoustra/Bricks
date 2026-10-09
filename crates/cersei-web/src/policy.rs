//! Where page downloads may go.
//!
//! Pages are fetched only over `http`/`https`, never from private, local,
//! link-local, multicast or otherwise non-public addresses — checked on the
//! URL of every hop (redirects included) for IP literals, and on every DNS
//! answer for host names, by the resolver the page client uses (so a name
//! that resolves to `127.0.0.1`, or changes its answer between two lookups,
//! is refused at connection time). Explicit exceptions (`allow_private`)
//! name a host or `host:port`; they exist for local servers and tests and are
//! distinct from the public default. Search-provider endpoints are validated
//! once at load instead (see `config`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use url::Url;

#[derive(Debug, Clone, Default)]
pub struct NetworkPolicy {
    /// `host` or `host:port`, lower-cased.
    allow_private: Vec<String>,
}

impl NetworkPolicy {
    pub fn new(allow_private: &[String]) -> Self {
        Self {
            allow_private: allow_private
                .iter()
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
        }
    }

    /// True when `host` (any port) or `host:port` is an explicit exception.
    pub fn allows_private(&self, host: &str, port: Option<u16>) -> bool {
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        self.allow_private.iter().any(|a| {
            let a = a.as_str();
            if a == host {
                return true;
            }
            match (port, a.rsplit_once(':')) {
                (Some(p), Some((h, ap))) => {
                    h.trim_start_matches('[').trim_end_matches(']') == host
                        && ap.parse::<u16>().ok() == Some(p)
                }
                _ => false,
            }
        })
    }

    /// Check one URL (a request or a redirect target).
    pub fn check_url(&self, url: &Url) -> Result<(), PolicyError> {
        match url.scheme() {
            "http" | "https" => {}
            s => return Err(PolicyError::Scheme(s.to_string())),
        }
        let host = url.host().ok_or(PolicyError::NoHost)?;
        let port = url.port_or_known_default();
        let ip = match host {
            url::Host::Ipv4(ip) => Some(IpAddr::V4(ip)),
            url::Host::Ipv6(ip) => Some(IpAddr::V6(ip)),
            url::Host::Domain(d) => {
                let d = d.trim_end_matches('.').to_ascii_lowercase();
                if (d == "localhost" || d.ends_with(".localhost")) && !self.allows_private(&d, port)
                {
                    return Err(PolicyError::Private(d));
                }
                None
            }
        };
        if let Some(ip) = ip {
            if !is_public(ip) && !self.allows_private(&ip.to_string(), port) {
                return Err(PolicyError::Private(ip.to_string()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("scheme `{0}` is not fetched (http and https only)")]
    Scheme(String),
    #[error("the URL has no host")]
    NoHost,
    #[error("{0} is a private, local or reserved address; it is not fetched (see web.fetch.allow_private)")]
    Private(String),
    #[error("{0} resolves only to private, local or reserved addresses; it is not fetched")]
    ResolvesPrivate(String),
}

/// Publicly routable unicast address.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = v6.segments();
            // NAT64 well-known prefix 64:ff9b::/96 embeds an IPv4 address.
            if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
                let v4 =
                    Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
                return is_public_v4(v4);
            }
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || (s[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x0100 && s[1..4] == [0, 0, 0]) // discard-only
                || v6 == Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0 // "this network"
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // shared address space 100.64/10
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // benchmarking 198.18/15
        || o[0] >= 240) // reserved
}

// ─── Resolver ────────────────────────────────────────────────────────────────

/// DNS for the page client: the shared resolver (`cersei_types::http`),
/// with answers filtered by the policy, so the check holds for the address
/// actually connected to.
#[derive(Clone)]
pub struct FilteringResolver {
    policy: Arc<NetworkPolicy>,
}

impl FilteringResolver {
    pub fn new(policy: Arc<NetworkPolicy>) -> Self {
        Self { policy }
    }
}

impl reqwest::dns::Resolve for FilteringResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let this = self.clone();
        Box::pin(async move {
            let host = name.as_str().trim_end_matches('.').to_ascii_lowercase();
            let lookup = cersei_types::http::lookup(host.as_str()).await?;
            // The port is not known here: an exception for `host` (any port)
            // lets every address through; `host:port` exceptions are checked
            // on the URL before the request.
            let allowed = this.policy.allows_private(&host, None);
            let addrs: Vec<SocketAddr> = lookup
                .into_iter()
                .filter(|ip| allowed || is_public(*ip))
                .map(|ip| SocketAddr::new(ip, 0))
                .collect();
            if addrs.is_empty() {
                return Err(Box::new(PolicyError::ResolvesPrivate(host))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_special_addresses_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "64:ff9b::a00:1",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "93.184.216.34",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn urls_are_checked_with_explicit_exceptions() {
        let p = NetworkPolicy::default();
        let ok = |u: &str| p.check_url(&Url::parse(u).unwrap());
        assert!(ok("https://example.com/a").is_ok());
        assert!(matches!(
            ok("http://127.0.0.1:8080/"),
            Err(PolicyError::Private(_))
        ));
        assert!(matches!(ok("http://[::1]/"), Err(PolicyError::Private(_))));
        assert!(matches!(
            ok("http://localhost/"),
            Err(PolicyError::Private(_))
        ));
        assert!(matches!(
            ok("http://169.254.169.254/latest"),
            Err(PolicyError::Private(_))
        ));
        assert!(matches!(
            ok("file:///etc/passwd"),
            Err(PolicyError::Scheme(_))
        ));
        assert!(matches!(
            ok("ftp://example.com/"),
            Err(PolicyError::Scheme(_))
        ));

        let p = NetworkPolicy::new(&["127.0.0.1:8080".into(), "localhost".into()]);
        assert!(p
            .check_url(&Url::parse("http://127.0.0.1:8080/x").unwrap())
            .is_ok());
        assert!(p
            .check_url(&Url::parse("http://127.0.0.1:9090/x").unwrap())
            .is_err());
        assert!(p
            .check_url(&Url::parse("http://localhost:1234/").unwrap())
            .is_ok());
    }
}
