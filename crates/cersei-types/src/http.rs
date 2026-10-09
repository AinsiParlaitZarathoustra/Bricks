//! HTTP clients and their DNS.
//!
//! Every HTTP client Bricks builds resolves names with hickory (pure Rust):
//! a static musl binary has no NSS for `getaddrinfo`, and the system
//! resolver would fail there without a word. One resolver per process,
//! from the system's configuration (`/etc/resolv.conf` on Unix), built at
//! the first lookup. Use [`client_builder`] (or [`client`]) instead of
//! `reqwest::Client::builder()`.
//!
//! reqwest's own `hickory-dns` feature is not used: it pins an older
//! hickory (RUSTSEC-2026-0118, RUSTSEC-2026-0119).
//!
//! When the system configuration cannot be read as a whole (hickory 0.26
//! refuses macOS's list when it holds a scoped link-local server such as
//! `fe80::1%en0`), the servers of `/etc/resolv.conf` that can be used are
//! taken instead, the scoped link-local ones left out. With none left, the
//! error is reported: no public resolver is ever substituted.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

static RESOLVER: tokio::sync::OnceCell<hickory_resolver::TokioResolver> =
    tokio::sync::OnceCell::const_new();

/// Name servers of a `resolv.conf` text that can be used as they are: IPv4
/// and IPv6 addresses; a zone (`%en0`) is dropped with its address when the
/// address is link-local (it cannot be reached without its interface).
pub fn usable_nameservers(resolv_conf: &str) -> Vec<IpAddr> {
    resolv_conf
        .lines()
        .filter_map(|l| {
            let l = l.split(['#', ';']).next()?.trim();
            let mut words = l.split_whitespace();
            (words.next()? == "nameserver").then(|| words.next())?
        })
        .filter_map(|addr| {
            let (ip, zone) = match addr.split_once('%') {
                Some((ip, zone)) => (ip, Some(zone)),
                None => (addr, None),
            };
            let ip: IpAddr = ip.parse().ok()?;
            let link_local = matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80);
            (zone.is_none() || !link_local).then_some(ip)
        })
        .collect()
}

fn builder() -> Result<
    hickory_resolver::ResolverBuilder<hickory_resolver::net::runtime::TokioRuntimeProvider>,
    BoxError,
> {
    match hickory_resolver::TokioResolver::builder_tokio() {
        Ok(b) => Ok(b),
        Err(system) => {
            let servers = std::fs::read_to_string("/etc/resolv.conf")
                .map(|t| usable_nameservers(&t))
                .unwrap_or_default();
            if servers.is_empty() {
                return Err(Box::new(system));
            }
            tracing::warn!(
                "DNS: the system configuration could not be read ({system}); using the {} usable \
                 server(s) of /etc/resolv.conf",
                servers.len()
            );
            let config = hickory_resolver::config::ResolverConfig::from_name_servers(
                servers
                    .into_iter()
                    .map(hickory_resolver::config::NameServerConfig::udp_and_tcp)
                    .collect(),
            );
            Ok(hickory_resolver::TokioResolver::builder_with_config(
                config,
                Default::default(),
            ))
        }
    }
}

/// The addresses of `host` (IPv4 and IPv6).
pub async fn lookup(host: &str) -> Result<Vec<IpAddr>, BoxError> {
    let resolver = RESOLVER
        .get_or_try_init(|| async {
            let mut b = builder()?;
            b.options_mut().ip_strategy = hickory_resolver::config::LookupIpStrategy::Ipv4AndIpv6;
            Ok::<_, BoxError>(b.build()?)
        })
        .await?;
    Ok(resolver.lookup_ip(host).await?.iter().collect())
}

/// reqwest's resolver: [`lookup`].
#[derive(Debug, Clone, Copy, Default)]
pub struct HickoryDns;

impl reqwest::dns::Resolve for HickoryDns {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = lookup(name.as_str())
                .await?
                .into_iter()
                .map(|ip| SocketAddr::new(ip, 0))
                .collect();
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A client builder that resolves names with [`HickoryDns`].
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().dns_resolver(Arc::new(HickoryDns))
}

/// A default client with [`HickoryDns`] (like `reqwest::Client::new()`,
/// which panics if TLS cannot be initialised).
pub fn client() -> reqwest::Client {
    client_builder()
        .build()
        .expect("the HTTP client (TLS backend) could not be initialised")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usable_servers_are_read_and_scoped_link_local_ones_left_out() {
        let conf = "# generated\nsearch home\nnameserver fe80::1%en0\nnameserver 2001:db8::53\n\
                    nameserver 192.168.1.1 # router\nnameserver not-an-ip\noptions ndots:1\n\
                    nameserver fd00::1%en1\n";
        let ips: Vec<String> = usable_nameservers(conf)
            .iter()
            .map(|i| i.to_string())
            .collect();
        assert_eq!(ips, vec!["2001:db8::53", "192.168.1.1", "fd00::1"]);
        assert!(usable_nameservers("nameserver fe80::1%en0\n").is_empty());
    }

    /// Literal addresses and `localhost` resolve without any network.
    #[tokio::test]
    async fn local_names_resolve_without_the_network() {
        let ips = lookup("127.0.0.1").await.unwrap();
        assert_eq!(ips, vec!["127.0.0.1".parse::<IpAddr>().unwrap()]);
        let ips = lookup("localhost").await.unwrap();
        assert!(ips.iter().all(|ip| ip.is_loopback()), "{ips:?}");
    }

    /// A public name through this machine's DNS (network: run with
    /// `--ignored`).
    #[tokio::test]
    #[ignore]
    async fn a_public_name_resolves_through_the_system_servers() {
        let ips = lookup("example.com").await.unwrap();
        assert!(!ips.is_empty());
    }

    /// A client built here talks to a local server through the shared
    /// resolver.
    #[tokio::test]
    async fn a_client_reaches_a_local_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf).await;
            let _ = s
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await;
        });
        let body = client()
            .get(format!("http://localhost:{port}/"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "ok");
    }
}
