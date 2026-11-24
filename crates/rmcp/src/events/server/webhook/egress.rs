use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use reqwest::{
    Client, Response, StatusCode, Url,
    dns::{Addrs, Name, Resolve, Resolving},
};
use tokio::sync::Semaphore;
use url::Host;

use super::{EventsError, WebhookSecret, invalid};
use crate::events::webhook::constant_time_eq;

const ATTEMPT: Duration = Duration::from_secs(5);

/// Additional outbound network and TLS trust for the Events webhook dispatcher.
///
/// The default policy accepts only globally routable destinations and uses
/// the platform trust store.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WebhookEgress {
    /// Additional allowed address prefixes, as address and prefix-length pairs.
    pub additional_allowed_networks: Vec<(IpAddr, u8)>,
    /// Extra DER-encoded CA certificates; hostname verification remains enabled.
    pub additional_root_certificates: Vec<Vec<u8>>,
}

pub(super) struct Egress {
    client: Client,
    policy: Arc<Policy>,
}

struct Policy {
    networks: Arc<[(IpAddr, u8)]>,
    lookups: Arc<Semaphore>,
}

impl Egress {
    pub(super) fn new(config: WebhookEgress) -> Result<Self, EventsError> {
        if config
            .additional_allowed_networks
            .iter()
            .any(|(addr, bits)| match addr {
                IpAddr::V4(_) => *bits > 32,
                IpAddr::V6(addr) => *bits > 128 || addr.to_ipv4_mapped().is_some(),
            })
        {
            return Err(invalid(
                "invalid webhook network prefix; use IPv4 for mapped addresses",
            ));
        }
        let policy = Arc::new(Policy {
            networks: config.additional_allowed_networks.into(),
            lookups: Arc::new(Semaphore::new(32)),
        });
        let mut builder = Client::builder()
            .tls_backend_rustls()
            .https_only(true)
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(15))
            .dns_resolver(policy.clone())
            .timeout(ATTEMPT);
        for der in config.additional_root_certificates {
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_der(&der)
                    .map_err(|_| invalid("invalid webhook root certificate"))?,
            );
        }
        let client = builder
            .build()
            .map_err(|_| invalid("could not configure webhook HTTPS client"))?;
        Ok(Self { client, policy })
    }

    pub(super) async fn post(
        &self,
        url: &Url,
        id: &str,
        message: &str,
        body: bytes::Bytes,
        secrets: (&WebhookSecret, Option<&WebhookSecret>),
    ) -> Result<Response, Reason> {
        let literal = match url.host() {
            Some(Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
            Some(Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
            _ => None,
        };
        if literal.is_some_and(|ip| !self.policy.allows(ip)) {
            return Err(Reason::ConnectionRefused);
        }
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let mut signature = secrets.0.signature(message, &timestamp, &body);
        if let Some(previous) = secrets.1 {
            signature.push(' ');
            signature.push_str(&previous.signature(message, &timestamp, &body));
        }
        let mut signature = http::HeaderValue::from_str(&signature).expect("ASCII signature");
        signature.set_sensitive(true);
        self.client
            .post(url.clone())
            .header("content-type", "application/json")
            .header("webhook-id", message)
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .header("x-mcp-subscription-id", id)
            .body(body)
            .send()
            .await
            .map_err(Reason::transport)
    }
}

pub(super) fn callback_url(url: &str) -> Result<Url, EventsError> {
    let invalid =
        || invalid("webhook callback must be HTTPS without credentials, fragment or port 0");
    let (scheme, rest) = url.split_once("://").ok_or_else(invalid)?;
    // Reject authority credentials even when empty and normalized away by URL parsing.
    if !scheme.eq_ignore_ascii_case("https")
        || rest.starts_with(['/', '\\'])
        || rest
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|authority| authority.contains(['@', '\\']))
        || url.bytes().any(|b| b.is_ascii_control())
    {
        return Err(invalid());
    }
    let parsed = Url::parse(url).map_err(|_| invalid())?;
    if parsed.scheme() != "https"
        || !parsed.has_host()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || parsed.port() == Some(0)
    {
        return Err(invalid());
    }
    Ok(parsed)
}

impl Resolve for Policy {
    fn resolve(&self, name: Name) -> Resolving {
        let networks = self.networks.clone();
        let lookups = self.lookups.clone();
        Box::pin(async move {
            tokio::time::timeout(Duration::from_secs(2), async move {
                let permit = lookups.acquire_owned().await?;
                // Dropping a waiter cannot interrupt a running OS lookup.
                // Keep its permit until the resolver task actually returns.
                let checked = tokio::spawn(async move {
                    let _permit = permit;
                    let addresses = tokio::net::lookup_host((name.as_str(), 0)).await?;
                    let checked: Vec<_> = addresses
                        .filter_map(|addr| {
                            let ip = addr.ip().to_canonical();
                            Policy::allows_ip(&networks, ip).then(|| {
                                if ip == addr.ip() {
                                    addr
                                } else {
                                    SocketAddr::new(ip, addr.port())
                                }
                            })
                        })
                        .collect();
                    if checked.is_empty() {
                        return Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
                    }
                    Ok(checked)
                })
                .await??;
                Ok(Box::new(checked.into_iter()) as Addrs)
            })
            .await
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
        })
    }
}

impl Policy {
    fn allows(&self, ip: IpAddr) -> bool {
        Self::allows_ip(&self.networks, ip.to_canonical())
    }

    fn allows_ip(prefixes: &[(IpAddr, u8)], ip: IpAddr) -> bool {
        prefixes.iter().any(|(net, bits)| match (*net, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                prefix(u32::from(net).into(), u32::from(ip).into(), 32, *bits)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => prefix(net.into(), ip.into(), 128, *bits),
            _ => false,
        }) || match ip {
            IpAddr::V4(ip) => global_v4(ip),
            IpAddr::V6(ip) => global_v6(ip),
        }
    }
}

fn prefix(network: u128, address: u128, width: u8, length: u8) -> bool {
    length == 0 || (network ^ address) >> (width - length) == 0
}

fn global_v4(ip: Ipv4Addr) -> bool {
    !matches!(ip.octets(),
        [0 | 10 | 127 | 224..=255, ..] |
        [100, 64..=127, ..] |
        [169, 254, ..] |
        [172, 16..=31, ..] |
        [192, 0, 0, 0..=8 | 11..=255] |
        [192, 0, 2, _] |
        [192, 88, 99, _] |
        [192, 168, ..] |
        [198, 18 | 19, ..] |
        [198, 51, 100, _] |
        [203, 0, 113, _]
    )
}

fn global_v6(ip: Ipv6Addr) -> bool {
    // Only public unicast allocations. Reject translation/tunneling ranges
    // by default too: their apparent destination can hide a private peer.
    match ip.segments() {
        [0x2001, 1, 0, 0, 0, 0, 0, 1..=3]
        | [0x2001, 3, ..]
        | [0x2001, 4, 0x112, ..]
        | [0x2001, 0x20..=0x3f, ..] => true,
        [0x2001, 0..=0x1ff | 0xdb8, ..] | [0x2002 | 0x3fff, ..] => false,
        [0x2000..=0x3ffe, ..] => true,
        _ => false,
    }
}

#[derive(Clone, Copy)]
pub(super) enum Reason {
    ConnectionRefused,
    Timeout,
    TlsError,
    Http4xx,
    Http5xx,
    ChallengeFailed,
}

impl Reason {
    pub(super) fn category(self) -> &'static str {
        match self {
            Self::ConnectionRefused => "connection_refused",
            Self::Timeout => "timeout",
            Self::TlsError => "tls_error",
            Self::Http4xx => "http_4xx",
            Self::Http5xx => "http_5xx",
            Self::ChallengeFailed => "challenge_failed",
        }
    }

    fn transport(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            return Self::Timeout;
        }
        let mut source = std::error::Error::source(&error);
        while let Some(error) = source {
            if let Some(io) = error.downcast_ref::<std::io::Error>() {
                if io.kind() == std::io::ErrorKind::InvalidData {
                    return Self::TlsError;
                }
                if io.kind() == std::io::ErrorKind::TimedOut {
                    return Self::Timeout;
                }
            }
            source = error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::get_ref)
                .map(|cause| cause as &dyn std::error::Error)
                .or_else(|| error.source());
        }
        Self::ConnectionRefused
    }

    pub(super) fn status(status: StatusCode) -> Self {
        if status.is_client_error() {
            Self::Http4xx
        } else if status.is_server_error() {
            Self::Http5xx
        } else {
            Self::ChallengeFailed
        }
    }
}

pub(super) async fn echo(mut response: Response, challenge: &str) -> Result<(), Reason> {
    if !response.status().is_success() {
        return Err(Reason::status(response.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Reason::transport)? {
        if chunk.len() > 1024 - body.len() {
            return Err(Reason::ChallengeFailed);
        }
        body.extend_from_slice(&chunk);
    }
    #[derive(serde::Deserialize)]
    struct Echo {
        challenge: String,
    }
    let echoed: Echo = serde_json::from_slice(&body).map_err(|_| Reason::ChallengeFailed)?;
    if constant_time_eq(challenge.as_bytes(), echoed.challenge.as_bytes()) {
        Ok(())
    } else {
        Err(Reason::ChallengeFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_egress_policy() {
        let mut policy = Policy {
            networks: Arc::from([]),
            lookups: Arc::new(Semaphore::new(1)),
        };
        for (address, allowed) in [
            ("0.1.2.3", false),
            ("10.0.0.1", false),
            ("100.63.255.255", true),
            ("100.64.0.0", false),
            ("100.127.255.255", false),
            ("100.128.0.0", true),
            ("127.0.0.1", false),
            ("169.254.1.1", false),
            ("172.31.0.1", false),
            ("192.0.0.8", false),
            ("192.0.0.9", true),
            ("192.0.0.10", true),
            ("192.0.0.11", false),
            ("192.0.2.1", false),
            ("192.88.99.1", false),
            ("192.168.0.1", false),
            ("198.18.0.0", false),
            ("198.19.255.255", false),
            ("198.20.0.0", true),
            ("198.51.100.1", false),
            ("203.0.113.1", false),
            ("224.0.0.1", false),
            ("240.0.0.1", false),
            ("255.255.255.255", false),
            ("::1", false),
            ("fe80::1", false),
            ("febf::1", false),
            ("fc00::1", false),
            ("fdff::1", false),
            ("64:ff9b::808:808", false),
            ("2002:808:808::1", false),
            ("2001:0:808:808::1", false),
            ("2001:db8::1", false),
            ("3fff::", false),
            ("2606:4700::1111", true),
            ("::ffff:127.0.0.1", false),
            ("::ffff:8.8.8.8", true),
        ] {
            assert_eq!(
                policy.allows(address.parse().unwrap()),
                allowed,
                "{address}"
            );
        }
        for (network, bits, address, allowed) in [
            ("0.0.0.0", 0, "10.0.0.1", true),
            ("0.0.0.0", 0, "::1", false),
            ("::", 0, "::1", true),
            ("::", 0, "10.0.0.1", false),
            ("127.0.0.1", 32, "::ffff:127.0.0.1", true),
            ("127.0.0.1", 32, "127.0.0.2", false),
            ("fc00::1", 128, "fc00::1", true),
            ("fc00::1", 128, "fc00::2", false),
        ] {
            policy.networks = Arc::from([(network.parse().unwrap(), bits)]);
            assert_eq!(
                policy.allows(address.parse().unwrap()),
                allowed,
                "{network}/{bits}: {address}"
            );
        }
        for url in [
            "http://example.com",
            "https://user:pass@example.com",
            "https://@example.com",
            "https://example.com/#fragment",
            "https://example.com:0",
            r"https://example.com\callback",
            "https://exam\tple.com",
            "https://example.com/\u{7f}",
        ] {
            assert!(callback_url(url).is_err(), "{url:?}");
        }
        assert!(callback_url("https://example.com:8443/callback?q=@").is_ok());
    }
}
