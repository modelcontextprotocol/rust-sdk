//! DNS resolution for OAuth HTTP requests that applies the metadata address
//! policy to the addresses the client actually connects to.
//!
//! Hosts named by discovered metadata may resolve to any public address. A
//! loopback or private address is allowed only when the base URL itself names
//! one: a loopback or private IP literal, or `localhost`. The base host's DNS
//! answers never widen the policy, since the resource may control them.
//! Link-local (including cloud instance metadata), unspecified, broadcast,
//! multicast and reserved addresses are never allowed. The base URL's own host
//! is filtered too, so it can't be rebound to an internal address. Configured
//! proxy hosts are not filtered.

use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::Arc,
};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tracing::warn;
use url::{Host, Url};

use super::AuthorizationManager;

const PROXY_ENV_VARS: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AddressClass {
    Public,
    Loopback,
    Private,
    Blocked,
}

impl AddressClass {
    pub(super) fn of(addr: IpAddr) -> Self {
        let addr = match addr {
            IpAddr::V6(v6) => {
                AuthorizationManager::embedded_ipv4(v6).map_or(IpAddr::V6(v6), IpAddr::V4)
            }
            v4 => v4,
        };
        match addr {
            IpAddr::V4(v4) => {
                let octets = v4.octets();
                if !AuthorizationManager::is_disallowed_metadata_ipv4(v4) {
                    Self::Public
                } else if v4.is_loopback() {
                    Self::Loopback
                } else if v4.is_private() || (octets[0] == 100 && (64..=127).contains(&octets[1])) {
                    Self::Private
                } else {
                    Self::Blocked
                }
            }
            IpAddr::V6(v6) => {
                if !AuthorizationManager::is_disallowed_metadata_ipv6(v6) {
                    Self::Public
                } else if v6.is_loopback() {
                    Self::Loopback
                } else if (v6.segments()[0] & 0xfe00) == 0xfc00 {
                    Self::Private
                } else {
                    Self::Blocked
                }
            }
        }
    }
}

/// Resolves names with the operating system resolver.
struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs =
                tokio::task::spawn_blocking(move || (host.as_str(), 0).to_socket_addrs()).await??;
            Ok(Box::new(addrs) as Addrs)
        })
    }
}

/// Filters resolved addresses by the class of the base URL's host.
#[derive(Clone)]
pub(super) struct MetadataAddressResolver {
    base_ip: Option<IpAddr>,
    unfiltered_hosts: Arc<[String]>,
    base_classes: Arc<[AddressClass]>,
    lookup: Arc<dyn Resolve>,
}

impl MetadataAddressResolver {
    pub(super) fn new(base_url: &Url) -> Self {
        Self::with_lookup(base_url, Arc::new(SystemResolver)).with_proxy_hosts(
            PROXY_ENV_VARS
                .iter()
                .filter_map(|var| std::env::var(var).ok()),
        )
    }

    pub(super) fn with_lookup(base_url: &Url, lookup: Arc<dyn Resolve>) -> Self {
        let base_ip = literal_ip(base_url);
        let base_classes = match base_url.host() {
            Some(Host::Ipv4(v4)) => vec![AddressClass::of(IpAddr::V4(v4))],
            Some(Host::Ipv6(v6)) => vec![AddressClass::of(IpAddr::V6(v6))],
            Some(Host::Domain(domain))
                if AuthorizationManager::is_loopback_metadata_host(Host::Domain(domain)) =>
            {
                vec![AddressClass::Loopback]
            }
            _ => Vec::new(),
        };
        Self {
            base_ip,
            unfiltered_hosts: Arc::new([]),
            base_classes: base_classes.into(),
            lookup,
        }
    }

    /// Exempts the hosts of proxy URLs, since a proxied request resolves only
    /// the proxy's name and the proxy resolves the target.
    pub(super) fn with_proxy_hosts(mut self, proxies: impl IntoIterator<Item = String>) -> Self {
        let proxy_hosts = proxies.into_iter().filter_map(|proxy| {
            let proxy = if proxy.contains("://") {
                proxy
            } else {
                format!("http://{proxy}")
            };
            Url::parse(&proxy).ok()?.host_str().map(normalize_host)
        });
        self.unfiltered_hosts = self
            .unfiltered_hosts
            .iter()
            .cloned()
            .chain(proxy_hosts)
            .collect();
        self
    }

    /// Also allows private addresses, whatever the base URL names.
    pub(super) fn allowing_private_addresses(mut self) -> Self {
        if !self.base_classes.contains(&AddressClass::Private) {
            self.base_classes = self
                .base_classes
                .iter()
                .copied()
                .chain([AddressClass::Private])
                .collect();
        }
        self
    }

    /// Applies the policy to an IP-literal host, which reqwest connects to
    /// without resolving. The base URL's own literal is always allowed.
    pub(super) fn allows_url(&self, url: &Url) -> bool {
        match literal_ip(url) {
            Some(ip) => {
                self.base_ip == Some(ip)
                    || Self::is_allowed(AddressClass::of(ip), &self.base_classes)
            }
            None => true,
        }
    }

    #[cfg(test)]
    pub(super) fn with_base_classes(mut self, classes: Vec<AddressClass>) -> Self {
        self.base_classes = classes.into();
        self
    }

    pub(super) fn is_allowed(class: AddressClass, base_classes: &[AddressClass]) -> bool {
        match class {
            AddressClass::Public => true,
            AddressClass::Loopback | AddressClass::Private => base_classes.contains(&class),
            AddressClass::Blocked => false,
        }
    }
}

impl Resolve for MetadataAddressResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let this = self.clone();
        Box::pin(async move {
            let host = normalize_host(name.as_str());
            let resolved: Vec<SocketAddr> = this.lookup.resolve(name).await?.collect();
            if this.unfiltered_hosts.contains(&host) {
                return Ok(Box::new(resolved.into_iter()) as Addrs);
            }

            let allowed: Vec<SocketAddr> = resolved
                .into_iter()
                .filter(|addr| Self::is_allowed(AddressClass::of(addr.ip()), &this.base_classes))
                .collect();
            if allowed.is_empty() {
                warn!("rejecting every resolved address of `{host}` for OAuth requests");
                return Err(format!(
                    "no address of `{host}` is allowed for OAuth requests from this resource"
                )
                .into());
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

fn literal_ip(url: &Url) -> Option<IpAddr> {
    match url.host()? {
        Host::Ipv4(v4) => Some(IpAddr::V4(v4)),
        Host::Ipv6(v6) => Some(IpAddr::V6(v6)),
        Host::Domain(_) => None,
    }
}

fn normalize_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use axum::{
        Router,
        http::{StatusCode, Uri, header},
        routing::get,
    };

    use super::*;
    use crate::transport::auth::{
        OAuthHttpClient, OAuthHttpRedirectPolicy, OAuthHttpRequest, ReqwestOAuthHttpClient,
    };

    /// Answers each name with its configured sequence, repeating the last answer.
    #[derive(Default)]
    struct StaticLookup(Mutex<HashMap<String, Vec<Vec<IpAddr>>>>);

    impl StaticLookup {
        fn with(entries: &[(&str, &[&[&str]])]) -> Arc<Self> {
            let map = entries
                .iter()
                .map(|(name, answers)| {
                    let answers = answers
                        .iter()
                        .map(|ips| ips.iter().map(|ip| ip.parse().unwrap()).collect())
                        .collect();
                    (name.to_string(), answers)
                })
                .collect();
            Arc::new(Self(Mutex::new(map)))
        }
    }

    impl Resolve for StaticLookup {
        fn resolve(&self, name: Name) -> Resolving {
            let mut map = self.0.lock().unwrap();
            let answer = match map.get_mut(name.as_str()) {
                Some(answers) if answers.len() > 1 => answers.remove(0),
                Some(answers) => answers[0].clone(),
                None => Vec::new(),
            };
            let addrs: Vec<SocketAddr> = answer.into_iter().map(|ip| (ip, 0).into()).collect();
            Box::pin(async move { Ok(Box::new(addrs.into_iter()) as Addrs) })
        }
    }

    async fn resolve(
        resolver: &MetadataAddressResolver,
        name: &str,
    ) -> Result<Vec<IpAddr>, String> {
        resolver
            .resolve(name.parse().unwrap())
            .await
            .map(|addrs| addrs.map(|addr| addr.ip()).collect())
            .map_err(|error| error.to_string())
    }

    fn resolver(base_url: &str, lookup: Arc<StaticLookup>) -> MetadataAddressResolver {
        MetadataAddressResolver::with_lookup(&Url::parse(base_url).unwrap(), lookup)
    }

    #[test]
    fn classifies_addresses() {
        let class = |ip: &str| AddressClass::of(ip.parse().unwrap());
        assert_eq!(class("93.184.216.34"), AddressClass::Public);
        assert_eq!(class("2606:4700::1111"), AddressClass::Public);
        assert_eq!(class("127.0.0.1"), AddressClass::Loopback);
        assert_eq!(class("::1"), AddressClass::Loopback);
        assert_eq!(class("::ffff:127.0.0.1"), AddressClass::Loopback);
        assert_eq!(class("10.0.0.9"), AddressClass::Private);
        assert_eq!(class("192.168.1.1"), AddressClass::Private);
        assert_eq!(class("100.64.0.1"), AddressClass::Private);
        assert_eq!(class("fd00::1"), AddressClass::Private);
        assert_eq!(class("169.254.169.254"), AddressClass::Blocked);
        assert_eq!(class("::ffff:169.254.169.254"), AddressClass::Blocked);
        assert_eq!(class("fe80::1"), AddressClass::Blocked);
        assert_eq!(class("0.0.0.0"), AddressClass::Blocked);
        assert_eq!(class("64:ff9b::a9fe:a9fe"), AddressClass::Blocked);
        assert_eq!(class("2002:a9fe:a9fe::"), AddressClass::Blocked);
        assert_eq!(class("::169.254.169.254"), AddressClass::Blocked);
        assert_eq!(class("2002:7f00:1::"), AddressClass::Loopback);
        assert_eq!(class("64:ff9b::a00:9"), AddressClass::Private);
        assert_eq!(class("64:ff9b::5db8:d822"), AddressClass::Public);
        assert_eq!(class("2001:0:4136:e378::1"), AddressClass::Blocked);
        assert_eq!(class("64:ff9b:1::a9fe:a9fe"), AddressClass::Blocked);
    }

    #[tokio::test]
    async fn public_resource_rejects_hosts_resolving_to_internal_addresses() {
        let lookup = StaticLookup::with(&[
            ("mcp.example.com", &[&["93.184.216.34"]]),
            ("loopback.attacker.test", &[&["127.0.0.1"]]),
            ("private.attacker.test", &[&["10.0.0.9"]]),
            ("imds.attacker.test", &[&["169.254.169.254"]]),
            ("auth.example.com", &[&["93.184.216.35"]]),
        ]);
        let resolver = resolver("https://mcp.example.com/mcp", lookup);

        for name in [
            "loopback.attacker.test",
            "private.attacker.test",
            "imds.attacker.test",
        ] {
            assert!(
                resolve(&resolver, name).await.is_err(),
                "{name} was allowed"
            );
        }
        assert_eq!(
            resolve(&resolver, "auth.example.com").await.unwrap(),
            vec!["93.184.216.35".parse::<IpAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn private_resource_allows_private_but_not_link_local_or_loopback() {
        let lookup = StaticLookup::with(&[
            ("sso.corp", &[&["10.0.0.9"]]),
            ("imds.corp", &[&["169.254.169.254"]]),
            ("loopback.corp", &[&["127.0.0.1"]]),
        ]);
        let resolver = resolver("https://10.0.0.4/mcp", lookup);

        assert!(resolve(&resolver, "sso.corp").await.is_ok());
        assert!(resolve(&resolver, "imds.corp").await.is_err());
        assert!(resolve(&resolver, "loopback.corp").await.is_err());
    }

    #[tokio::test]
    async fn base_host_answers_do_not_widen_the_policy() {
        let lookup = StaticLookup::with(&[
            (
                "mcp.attacker.test",
                &[&["93.184.216.34", "127.0.0.1", "10.0.0.1"]],
            ),
            ("loopback.attacker.test", &[&["127.0.0.1"]]),
            ("private.attacker.test", &[&["10.0.0.9"]]),
        ]);
        let resolver = resolver("https://mcp.attacker.test/mcp", lookup);

        assert!(resolve(&resolver, "loopback.attacker.test").await.is_err());
        assert!(resolve(&resolver, "private.attacker.test").await.is_err());
    }

    #[test]
    fn ip_literal_urls_follow_the_policy() {
        let allows = |base: &str, url: &str| {
            resolver(base, StaticLookup::with(&[])).allows_url(&Url::parse(url).unwrap())
        };

        for url in [
            "http://127.0.0.1:6379/",
            "http://[::1]:6379/",
            "http://10.0.0.1/token",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            assert!(
                !allows("https://mcp.example.com/mcp", url),
                "{url} was allowed"
            );
        }
        assert!(allows(
            "https://mcp.example.com/mcp",
            "https://93.184.216.35/token"
        ));
        assert!(allows(
            "https://mcp.example.com/mcp",
            "https://auth.example.com/token"
        ));
        assert!(allows(
            "http://127.0.0.1:3000/mcp",
            "http://127.0.0.2:8080/token"
        ));
        assert!(allows(
            "http://0.0.0.0:3000/mcp",
            "http://0.0.0.0:3000/register"
        ));
        assert!(!allows(
            "http://0.0.0.0:3000/mcp",
            "http://169.254.169.254/"
        ));
    }

    #[tokio::test]
    async fn allowing_private_addresses_keeps_the_rest_of_the_policy() {
        let lookup = StaticLookup::with(&[
            ("sso.corp", &[&["10.0.0.9"]]),
            ("loopback.corp", &[&["127.0.0.1"]]),
            ("imds.corp", &[&["169.254.169.254"]]),
        ]);
        let public = resolver("https://mcp.corp/mcp", lookup.clone()).allowing_private_addresses();
        let loopback = resolver("http://localhost:3000/mcp", lookup).allowing_private_addresses();

        assert!(resolve(&public, "sso.corp").await.is_ok());
        assert!(resolve(&public, "loopback.corp").await.is_err());
        assert!(resolve(&public, "imds.corp").await.is_err());
        assert!(resolve(&loopback, "sso.corp").await.is_ok());
        assert!(resolve(&loopback, "loopback.corp").await.is_ok());
        assert!(!public.allows_url(&Url::parse("http://169.254.169.254/").unwrap()));
    }

    #[tokio::test]
    async fn proxy_hosts_are_not_filtered() {
        let lookup = StaticLookup::with(&[
            ("mcp.example.com", &[&["93.184.216.34"]]),
            ("proxy.corp", &[&["10.0.0.3"]]),
            ("bare-proxy.corp", &[&["10.0.0.4"]]),
            ("private.attacker.test", &[&["10.0.0.9"]]),
        ]);
        let resolver = resolver("https://mcp.example.com/mcp", lookup).with_proxy_hosts([
            "http://user:pass@Proxy.Corp:3128".to_string(),
            "bare-proxy.corp:8080".to_string(),
        ]);

        assert!(resolve(&resolver, "proxy.corp").await.is_ok());
        assert!(resolve(&resolver, "bare-proxy.corp").await.is_ok());
        assert!(resolve(&resolver, "private.attacker.test").await.is_err());
    }

    #[tokio::test]
    async fn loopback_resource_allows_loopback_but_not_private() {
        let lookup = StaticLookup::with(&[
            ("localhost", &[&["127.0.0.1", "::1"]]),
            ("auth.localhost", &[&["127.0.0.1"]]),
            ("private.test", &[&["192.168.1.10"]]),
        ]);
        let literal = resolver("http://127.0.0.1:8080/mcp", lookup.clone());
        let named = resolver("http://localhost:8080/mcp", lookup);

        for resolver in [&literal, &named] {
            assert!(resolve(resolver, "auth.localhost").await.is_ok());
            assert!(resolve(resolver, "private.test").await.is_err());
        }
    }

    #[tokio::test]
    async fn base_host_cannot_rebind_to_internal_addresses() {
        let lookup = StaticLookup::with(&[(
            "mcp.attacker.test",
            &[&["93.184.216.34"], &["127.0.0.1"], &["169.254.169.254"]],
        )]);
        let resolver = resolver("https://MCP.attacker.test./mcp", lookup);

        assert!(resolve(&resolver, "mcp.attacker.test").await.is_ok());
        assert!(resolve(&resolver, "mcp.attacker.test").await.is_err());
        assert!(resolve(&resolver, "mcp.attacker.test").await.is_err());
    }

    #[tokio::test]
    async fn mixed_answers_keep_only_allowed_addresses() {
        let lookup = StaticLookup::with(&[
            ("mcp.example.com", &[&["93.184.216.34"]]),
            (
                "auth.attacker.test",
                &[&["127.0.0.1", "93.184.216.35", "10.0.0.1"]],
            ),
        ]);
        let resolver = resolver("https://mcp.example.com/mcp", lookup);

        assert_eq!(
            resolve(&resolver, "auth.attacker.test").await.unwrap(),
            vec!["93.184.216.35".parse::<IpAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn every_resolution_is_filtered() {
        let lookup = StaticLookup::with(&[
            ("mcp.example.com", &[&["93.184.216.34"]]),
            (
                "rebind.attacker.test",
                &[&["93.184.216.35"], &["127.0.0.1"]],
            ),
        ]);
        let resolver = resolver("https://mcp.example.com/mcp", lookup);

        assert!(resolve(&resolver, "rebind.attacker.test").await.is_ok());
        assert!(resolve(&resolver, "rebind.attacker.test").await.is_err());
    }

    const RESOURCE_HOST: &str = "mcp.attacker.test";
    const INTERNAL_HOST: &str = "internal.attacker.test";

    /// An internal service on loopback that records every request it receives.
    async fn spawn_internal_service() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let recorded = hits.clone();
        let app = Router::new().fallback(move |uri: Uri| {
            let recorded = recorded.clone();
            async move {
                recorded.lock().unwrap().push(uri.to_string());
                (StatusCode::NOT_FOUND, "internal")
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (addr, hits)
    }

    /// A resource whose same-origin protected resource metadata names
    /// `authorization_server`.
    async fn spawn_resource(authorization_server: String) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let resource = format!("http://{RESOURCE_HOST}:{}/mcp", addr.port());
        let app = Router::new()
            .route(
                "/mcp",
                get(|| async {
                    (
                        StatusCode::UNAUTHORIZED,
                        [(
                            header::WWW_AUTHENTICATE,
                            r#"Bearer resource_metadata="/.well-known/oauth-protected-resource""#,
                        )],
                    )
                }),
            )
            .route(
                "/.well-known/oauth-protected-resource",
                get(move || {
                    let body = serde_json::json!({
                        "resource": resource,
                        "authorization_servers": [authorization_server],
                    });
                    async move {
                        (
                            [(header::CONTENT_TYPE, "application/json")],
                            body.to_string(),
                        )
                    }
                }),
            );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    /// Runs discovery through the SDK's reqwest client and returns the requests
    /// that reached the internal service. `resource_class` is the class the
    /// resource's host is treated as resolving to; both servers listen on loopback,
    /// so the resource's host is exempted to stand in for a reachable resource.
    async fn discover(
        resource_class: AddressClass,
        authorization_server: impl Fn(SocketAddr) -> String,
    ) -> Vec<String> {
        let (internal, hits) = spawn_internal_service().await;
        let resource = spawn_resource(authorization_server(internal)).await;
        let base_url =
            Url::parse(&format!("http://{RESOURCE_HOST}:{}/mcp", resource.port())).unwrap();
        let lookup = StaticLookup::with(&[
            (RESOURCE_HOST, &[&["127.0.0.1"]]),
            (INTERNAL_HOST, &[&["127.0.0.1"]]),
        ]);
        let resolver = MetadataAddressResolver::with_lookup(&base_url, lookup)
            .with_proxy_hosts([RESOURCE_HOST.to_string()])
            .with_base_classes(vec![resource_class]);
        let client = ReqwestOAuthHttpClient::with_resolver(Arc::new(resolver)).unwrap();
        let manager = AuthorizationManager::new_inner(
            base_url,
            Arc::new(client),
            OAuthHttpRedirectPolicy::Stop,
        )
        .await
        .unwrap();

        let _ = manager.resolve_metadata().await;
        hits.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn public_resource_cannot_reach_internal_service_by_literal_address() {
        let hits = discover(AddressClass::Public, |internal| {
            format!("http://127.0.0.1:{}/", internal.port())
        })
        .await;
        assert!(hits.is_empty(), "internal service received {hits:?}");
    }

    #[tokio::test]
    async fn public_resource_cannot_reach_internal_service_by_hostname() {
        let hits = discover(AddressClass::Public, |internal| {
            format!("http://{INTERNAL_HOST}:{}/", internal.port())
        })
        .await;
        assert!(hits.is_empty(), "internal service received {hits:?}");
    }

    #[tokio::test]
    async fn loopback_resource_reaches_loopback_authorization_server_by_hostname() {
        let hits = discover(AddressClass::Loopback, |internal| {
            format!("http://{INTERNAL_HOST}:{}/", internal.port())
        })
        .await;
        assert!(
            !hits.is_empty(),
            "loopback authorization server was not queried"
        );
    }
    /// A reqwest client for a public resource at `RESOURCE_HOST`, which is
    /// exempted to stand in for a reachable resource on loopback.
    fn public_resource_client() -> ReqwestOAuthHttpClient {
        let base_url = Url::parse(&format!("http://{RESOURCE_HOST}/mcp")).unwrap();
        let lookup = StaticLookup::with(&[(RESOURCE_HOST, &[&["127.0.0.1"]])]);
        let resolver = MetadataAddressResolver::with_lookup(&base_url, lookup)
            .with_proxy_hosts([RESOURCE_HOST.to_string()]);
        ReqwestOAuthHttpClient::with_resolver(Arc::new(resolver)).unwrap()
    }

    fn assert_rejected(error: crate::transport::auth::OAuthHttpClientError) {
        let mut source: Option<&dyn std::error::Error> = Some(error.as_ref());
        let mut chain = Vec::new();
        while let Some(error) = source {
            chain.push(error.to_string());
            source = error.source();
        }
        assert!(
            chain
                .iter()
                .any(|message| message.contains("address not allowed")),
            "unexpected error: {chain:?}"
        );
    }

    fn post(url: String, redirect_policy: OAuthHttpRedirectPolicy) -> OAuthHttpRequest {
        let request = oauth2::http::Request::builder()
            .method("POST")
            .uri(url)
            .body(Vec::new())
            .unwrap();
        OAuthHttpRequest::new(request, redirect_policy)
    }

    #[tokio::test]
    async fn public_resource_cannot_post_to_internal_ip_literal() {
        let (internal, hits) = spawn_internal_service().await;
        let client = public_resource_client();

        let result = client
            .execute(post(
                format!("http://127.0.0.1:{}/token", internal.port()),
                OAuthHttpRedirectPolicy::Stop,
            ))
            .await;

        assert_rejected(result.unwrap_err());
        assert!(hits.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn public_resource_cannot_be_redirected_to_internal_ip_literal() {
        let (internal, hits) = spawn_internal_service().await;
        let target = format!("http://127.0.0.1:{}/token", internal.port());
        let app = Router::new().fallback(move || {
            let target = target.clone();
            async move { (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, target)]) }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirector = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = public_resource_client();

        let result = client
            .execute(post(
                format!("http://{RESOURCE_HOST}:{}/token", redirector.port()),
                OAuthHttpRedirectPolicy::Follow,
            ))
            .await;

        assert_rejected(result.unwrap_err());
        assert!(hits.lock().unwrap().is_empty());
    }
}
