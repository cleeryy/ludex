//! Outbound URL policy for fetches of user- or provider-supplied URLs.
//!
//! Every outbound request made on behalf of a provider, a webhook target or a
//! library file must go through [`UrlGuard::get`]. The guard:
//!
//! 1. accepts only `http`/`https` URLs without embedded credentials;
//! 2. resolves the host and rejects private, loopback, link-local, CGNAT,
//!    documentation, multicast and unspecified addresses (IPv4 and IPv6,
//!    including IPv4-mapped IPv6);
//! 3. disables reqwest's automatic redirects and follows `Location` hop by hop,
//!    re-validating every hop against the same policy, up to a small limit;
//! 4. pins the connection to the address that was validated (`reqwest`'s
//!    `resolve` override), so a second DNS answer cannot point the request at a
//!    private network, and keeps the original `Host` header;
//! 5. caps the response body while streaming and applies a timeout.
//!
//! Error strings never include the full URL: query strings may carry signed
//! tokens, and the guard's errors are logged by callers.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use url::Url;

/// Redirects followed by default before giving up.
pub const MAX_REDIRECTS: usize = 5;
/// Default response body cap (25 MiB, the artwork limit).
pub const MAX_BYTES: usize = 25 * 1024 * 1024;
/// Default whole-request timeout.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// Connect timeout for a single hop.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Boxed future returned by [`Resolver`] and [`Transport`].
pub type GuardFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Resolves a host name to socket addresses.
///
/// A trait instead of a free function so tests can inject a deterministic
/// answer and probe the DNS-pinning path without touching the network.
pub trait Resolver: Send + Sync {
    /// Resolves `host` on `port`.
    fn resolve(&self, host: String, port: u16)
        -> GuardFuture<'_, std::io::Result<Vec<SocketAddr>>>;
}

/// Resolver backed by the system DNS configuration.
#[derive(Debug, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve(
        &self,
        host: String,
        port: u16,
    ) -> GuardFuture<'_, std::io::Result<Vec<SocketAddr>>> {
        Box::pin(async move {
            let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
                .await?
                .collect();
            Ok(addresses)
        })
    }
}

/// One HTTP exchange, before redirect handling and body limits are applied.
#[derive(Debug, Clone)]
pub struct RawResponse {
    /// HTTP status code.
    pub status: u16,
    /// `Location` header for redirect responses.
    pub location: Option<String>,
    /// `Content-Type` header, verbatim.
    pub content_type: Option<String>,
    /// Response body (empty for redirect responses).
    pub body: Vec<u8>,
}

/// A fully validated request: the URL to send, the address to connect to and
/// the limits to enforce.
#[derive(Debug)]
pub struct GuardedRequest {
    /// Request URL; the host stays the original one so `Host` and TLS SNI match.
    pub url: Url,
    /// Validated address the connection must go to.
    pub address: SocketAddr,
    /// Whole-request timeout.
    pub timeout: Duration,
    /// Maximum body size.
    pub max_bytes: usize,
}

/// Performs a single HTTP GET against a pinned address.
pub trait Transport: Send + Sync {
    /// Fetches `request`, without following redirects.
    fn fetch(&self, request: GuardedRequest) -> GuardFuture<'_, Result<RawResponse, GuardError>>;
}

/// Transport backed by `reqwest` with automatic redirects disabled.
#[derive(Debug, Default)]
pub struct ReqwestTransport;

impl Transport for ReqwestTransport {
    fn fetch(&self, request: GuardedRequest) -> GuardFuture<'_, Result<RawResponse, GuardError>> {
        Box::pin(async move {
            let host = request
                .url
                .host_str()
                .ok_or(GuardError::MissingHost)?
                .to_string();
            let mut builder = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(request.timeout)
                .connect_timeout(CONNECT_TIMEOUT)
                .user_agent(concat!("Ludex/", env!("CARGO_PKG_VERSION")));
            // Pin the name to the address that passed the private-IP check.
            // IP literals need no override: they are their own address.
            if request
                .url
                .host()
                .is_some_and(|host| matches!(host, url::Host::Domain(_)))
            {
                builder = builder.resolve(&host, request.address);
            }
            let client = builder
                .build()
                .map_err(|error| GuardError::Client(error.without_url().to_string()))?;
            let response = client
                .get(request.url)
                .send()
                .await
                .map_err(|error| GuardError::Request(error.without_url().to_string()))?;
            let status = response.status().as_u16();
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            if is_redirect(status) {
                return Ok(RawResponse {
                    status,
                    location,
                    content_type,
                    body: Vec::new(),
                });
            }
            if response
                .content_length()
                .is_some_and(|length| length > request.max_bytes as u64)
            {
                return Err(GuardError::TooLarge(request.max_bytes));
            }
            let mut body = Vec::new();
            let mut response = response;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| GuardError::Request(error.without_url().to_string()))?
            {
                if body.len().saturating_add(chunk.len()) > request.max_bytes {
                    return Err(GuardError::TooLarge(request.max_bytes));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(RawResponse {
                status,
                location,
                content_type,
                body,
            })
        })
    }
}

/// A fetch that passed the guard.
#[derive(Debug, Clone)]
pub struct Fetched {
    /// URL of the final hop.
    pub url: Url,
    /// HTTP status of the final hop.
    pub status: u16,
    /// `Content-Type` of the final hop, when present.
    pub content_type: Option<String>,
    /// Response body, capped at the guard's limit.
    pub body: Vec<u8>,
}

/// A guarded fetch failure.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    /// The URL cannot be parsed.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    /// The scheme is not `http` or `https`.
    #[error("only http and https URLs are allowed")]
    UnsupportedScheme,
    /// The URL embeds user info.
    #[error("URLs with embedded credentials are not allowed")]
    Credentials,
    /// The URL has no host.
    #[error("URL has no host")]
    MissingHost,
    /// DNS resolution failed.
    #[error("could not resolve {host}: {source}")]
    Resolve {
        /// Host that failed to resolve.
        host: String,
        /// Underlying resolver error.
        #[source]
        source: std::io::Error,
    },
    /// DNS returned no addresses.
    #[error("no addresses resolved for {0}")]
    NoAddresses(String),
    /// The target (or one of its answers) is not a public address.
    #[error("refusing to fetch the private address {0}")]
    PrivateAddress(IpAddr),
    /// The redirect chain exceeded the guard's limit.
    #[error("redirect limit exceeded")]
    TooManyRedirects,
    /// A redirect response had no usable `Location` header.
    #[error("redirect response has no usable Location header")]
    RedirectWithoutLocation,
    /// The `Location` value did not resolve to an absolute http(s) URL.
    #[error("invalid redirect target")]
    InvalidRedirect,
    /// The transport failed.
    #[error("HTTP request failed: {0}")]
    Request(String),
    /// The response body exceeded the guard's cap.
    #[error("response exceeds {0} bytes")]
    TooLarge(usize),
    /// The HTTP client could not be built.
    #[error("could not build HTTP client: {0}")]
    Client(String),
}

/// Whether an HTTP status is a redirect the guard should follow.
#[must_use]
pub fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Shared private-address policy.
///
/// Rejects loopback, RFC 1918, link-local (including the cloud metadata
/// address `169.254.169.254`), CGNAT (`100.64.0.0/10`), documentation,
/// benchmark, reserved, multicast and unspecified ranges. IPv6 covers
/// loopback, unspecified, ULA, link-local, site-local and IPv4-mapped or
/// IPv4-compatible addresses.
#[must_use]
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_private_ipv4(ip),
        IpAddr::V6(ip) => is_private_ipv6(ip),
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        // 0.0.0.0/8 is "this network" and routes to localhost on Linux.
        || first == 0
        // 100.64.0.0/10 carrier-grade NAT.
        || (first == 100 && (64..=127).contains(&second))
        // 198.18.0.0/15 benchmarking.
        || (first == 198 && matches!(second, 18 | 19))
        // 192.0.0.0/24 IETF protocol assignments.
        || (first == 192 && second == 0 && ip.octets()[2] == 0)
        // 240.0.0.0/4 reserved.
        || first >= 240
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_private_ipv4(mapped);
    }
    if let Some(compatible) = ip.to_ipv4() {
        return is_private_ipv4(compatible);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        // Deprecated fec0::/10 site-local.
        || (first & 0xffc0) == 0xfec0
}

/// Validates the shape of an outbound URL.
///
/// # Errors
///
/// Returns [`GuardError::InvalidUrl`] when parsing fails and
/// [`GuardError`] scheme/credential/host variants for policy violations.
pub fn validate_url(raw: &str) -> Result<Url, GuardError> {
    let url = Url::parse(raw).map_err(|error| GuardError::InvalidUrl(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(GuardError::UnsupportedScheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(GuardError::Credentials);
    }
    if url.host_str().is_none() {
        return Err(GuardError::MissingHost);
    }
    Ok(url)
}

/// Resolves a redirect target against the current URL and validates it.
fn redirect_target(current: &Url, location: &str) -> Result<Url, GuardError> {
    let target = current
        .join(location)
        .map_err(|_| GuardError::InvalidRedirect)?;
    validate_url(target.as_str())
}

/// The address a hop should connect to. IPv4 is preferred because it is
/// reachable on more deployments than IPv6.
fn preferred_address(addresses: &[SocketAddr]) -> SocketAddr {
    addresses
        .iter()
        .find(|address| address.is_ipv4())
        .copied()
        .unwrap_or(addresses[0])
}

/// Applies the outbound URL policy.
#[derive(Clone)]
pub struct UrlGuard {
    allow_private: bool,
    max_redirects: usize,
    max_bytes: usize,
    timeout: Duration,
    resolver: Arc<dyn Resolver>,
    transport: Arc<dyn Transport>,
}

impl std::fmt::Debug for UrlGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UrlGuard")
            .field("allow_private", &self.allow_private)
            .field("max_redirects", &self.max_redirects)
            .field("max_bytes", &self.max_bytes)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl UrlGuard {
    /// Builds a guard with the default limits and the system DNS resolver.
    #[must_use]
    pub fn new(allow_private: bool) -> Self {
        Self {
            allow_private,
            max_redirects: MAX_REDIRECTS,
            max_bytes: MAX_BYTES,
            timeout: TIMEOUT,
            resolver: Arc::new(SystemResolver),
            transport: Arc::new(ReqwestTransport),
        }
    }

    /// Overrides the redirect limit, body cap and timeout.
    #[must_use]
    pub fn with_limits(
        mut self,
        max_redirects: usize,
        max_bytes: usize,
        timeout: Duration,
    ) -> Self {
        self.max_redirects = max_redirects;
        self.max_bytes = max_bytes;
        self.timeout = timeout;
        self
    }

    /// Injects a resolver (tests).
    #[must_use]
    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.resolver = resolver;
        self
    }

    /// Injects a transport (tests).
    #[must_use]
    pub fn with_transport(mut self, transport: Arc<dyn Transport>) -> Self {
        self.transport = transport;
        self
    }

    /// Resolves `url` and applies the private-address policy.
    ///
    /// Every address returned by DNS must be public: a mixed answer is
    /// rejected instead of picking a public one, because the private entry is
    /// a strong signal of a rebinding attempt.
    ///
    /// # Errors
    ///
    /// Returns [`GuardError::PrivateAddress`] when any resolved address is
    /// local, and the resolution variants when DNS fails.
    pub async fn resolve(&self, url: &Url) -> Result<Vec<SocketAddr>, GuardError> {
        let host = url.host_str().ok_or(GuardError::MissingHost)?.to_string();
        let port = url.port_or_known_default().ok_or(GuardError::MissingHost)?;
        let addresses = match url.host() {
            Some(url::Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
            Some(url::Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
            Some(url::Host::Domain(domain)) => self
                .resolver
                .resolve(domain.to_string(), port)
                .await
                .map_err(|source| GuardError::Resolve {
                    host: domain.to_string(),
                    source,
                })?,
            None => return Err(GuardError::MissingHost),
        };
        if addresses.is_empty() {
            return Err(GuardError::NoAddresses(host));
        }
        if !self.allow_private {
            for address in &addresses {
                if is_private_ip(address.ip()) {
                    return Err(GuardError::PrivateAddress(address.ip()));
                }
            }
        }
        Ok(addresses)
    }

    /// Fetches `raw`, following redirects manually with a per-hop policy check.
    ///
    /// The response status is returned as-is: callers decide whether a
    /// non-2xx status is an error.
    ///
    /// # Errors
    ///
    /// Returns a [`GuardError`] when the URL, a redirect hop or the body
    /// violates the policy.
    pub async fn get(&self, raw: &str) -> Result<Fetched, GuardError> {
        let mut url = validate_url(raw)?;
        for hop in 0..=self.max_redirects {
            let addresses = self.resolve(&url).await?;
            let response = self
                .transport
                .fetch(GuardedRequest {
                    url: url.clone(),
                    address: preferred_address(&addresses),
                    timeout: self.timeout,
                    max_bytes: self.max_bytes,
                })
                .await?;
            if is_redirect(response.status) {
                let location = response
                    .location
                    .ok_or(GuardError::RedirectWithoutLocation)?;
                if hop == self.max_redirects {
                    return Err(GuardError::TooManyRedirects);
                }
                url = redirect_target(&url, &location)?;
                continue;
            }
            if response.body.len() > self.max_bytes {
                return Err(GuardError::TooLarge(self.max_bytes));
            }
            return Ok(Fetched {
                url,
                status: response.status,
                content_type: response.content_type,
                body: response.body,
            });
        }
        Err(GuardError::TooManyRedirects)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    struct StaticResolver {
        answers: HashMap<String, Vec<IpAddr>>,
    }

    impl StaticResolver {
        fn new(entries: &[(&str, &str)]) -> Self {
            Self {
                answers: entries
                    .iter()
                    .map(|(host, ip)| {
                        ((*host).to_string(), vec![ip.parse().expect("test address")])
                    })
                    .collect(),
            }
        }
    }

    impl Resolver for StaticResolver {
        fn resolve(
            &self,
            host: String,
            port: u16,
        ) -> GuardFuture<'_, std::io::Result<Vec<SocketAddr>>> {
            let answer = self.answers.get(&host).map(|addresses| {
                addresses
                    .iter()
                    .map(|ip| SocketAddr::new(*ip, port))
                    .collect::<Vec<_>>()
            });
            Box::pin(async move {
                answer.ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "unknown test host")
                })
            })
        }
    }

    /// Serves canned responses keyed by URL and records the addresses the
    /// guard asked for, so pinning can be asserted without a network.
    #[derive(Default)]
    struct FakeTransport {
        responses: Mutex<HashMap<String, RawResponse>>,
        requested: Mutex<Vec<(String, SocketAddr)>>,
    }

    impl FakeTransport {
        fn with(responses: &[(&str, RawResponse)]) -> Self {
            Self {
                responses: Mutex::new(
                    responses
                        .iter()
                        .map(|(url, response)| ((*url).to_string(), response.clone()))
                        .collect(),
                ),
                requested: Mutex::new(Vec::new()),
            }
        }
    }

    impl Transport for FakeTransport {
        fn fetch(
            &self,
            request: GuardedRequest,
        ) -> GuardFuture<'_, Result<RawResponse, GuardError>> {
            let key = request.url.to_string();
            let response = self
                .responses
                .lock()
                .expect("lock")
                .remove(&key)
                .ok_or_else(|| GuardError::Request(format!("unexpected request for {key}")));
            self.requested
                .lock()
                .expect("lock")
                .push((key, request.address));
            Box::pin(async move { response })
        }
    }

    fn ok(body: &str) -> RawResponse {
        RawResponse {
            status: 200,
            location: None,
            content_type: Some("image/jpeg".to_string()),
            body: body.as_bytes().to_vec(),
        }
    }

    fn redirect(location: &str) -> RawResponse {
        RawResponse {
            status: 302,
            location: Some(location.to_string()),
            content_type: None,
            body: Vec::new(),
        }
    }

    fn guard() -> UrlGuard {
        UrlGuard::new(false)
    }

    #[tokio::test]
    async fn public_url_is_allowed() {
        let resolver = Arc::new(StaticResolver::new(&[(
            "images.example.com",
            "93.184.216.34",
        )]));
        let transport = Arc::new(FakeTransport::with(&[(
            "https://images.example.com/cover.jpg",
            ok("image"),
        )]));
        let guard = guard()
            .with_resolver(resolver)
            .with_transport(transport.clone());
        let fetched = guard
            .get("https://images.example.com/cover.jpg")
            .await
            .expect("public artwork allowed");
        assert_eq!(fetched.status, 200);
        assert_eq!(fetched.body, b"image");
        assert_eq!(fetched.content_type.as_deref(), Some("image/jpeg"));
        let requested = transport.requested.lock().expect("lock");
        assert_eq!(
            requested[0].1,
            "93.184.216.34:443".parse().expect("socket address"),
            "the validated address must be pinned"
        );
    }

    #[tokio::test]
    async fn private_addresses_are_rejected() {
        for raw in [
            "http://127.0.0.1/cover.jpg",
            "http://10.1.50.103/cover.jpg",
            "http://192.168.1.20/cover.jpg",
            "http://172.16.4.9/cover.jpg",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.64.0.1/cover.jpg",
            "http://0.0.0.0/cover.jpg",
            "http://224.0.0.1/cover.jpg",
            "http://198.18.0.5/cover.jpg",
            "http://240.0.0.1/cover.jpg",
            "http://[::1]/cover.jpg",
            "http://[fc00::1]/cover.jpg",
            "http://[fd12:3456::1]/cover.jpg",
            "http://[fe80::1]/cover.jpg",
            "http://[::ffff:127.0.0.1]/cover.jpg",
            "http://[::ffff:10.0.0.5]/cover.jpg",
            "http://[::]/cover.jpg",
        ] {
            let transport = Arc::new(FakeTransport::default());
            let error = guard()
                .with_transport(transport.clone())
                .get(raw)
                .await
                .expect_err(raw);
            assert!(
                matches!(error, GuardError::PrivateAddress(_)),
                "{raw}: {error}"
            );
            assert!(
                transport.requested.lock().expect("lock").is_empty(),
                "{raw}: no request may be sent"
            );
        }
    }

    #[tokio::test]
    async fn hostname_resolving_to_private_is_rejected() {
        let resolver = Arc::new(StaticResolver::new(&[
            ("metadata.internal", "169.254.169.254"),
            ("mixed.example", "93.184.216.34"),
        ]));
        let error = guard()
            .with_resolver(resolver)
            .get("http://metadata.internal/latest")
            .await
            .expect_err("private DNS answer");
        assert!(matches!(error, GuardError::PrivateAddress(_)), "{error}");
    }

    #[tokio::test]
    async fn mixed_dns_answers_are_rejected() {
        struct MixedResolver;
        impl Resolver for MixedResolver {
            fn resolve(
                &self,
                _host: String,
                port: u16,
            ) -> GuardFuture<'_, std::io::Result<Vec<SocketAddr>>> {
                Box::pin(async move {
                    Ok(vec![
                        SocketAddr::new("93.184.216.34".parse().expect("public"), port),
                        SocketAddr::new("10.0.0.5".parse().expect("private"), port),
                    ])
                })
            }
        }
        let error = guard()
            .with_resolver(Arc::new(MixedResolver))
            .get("http://mixed.example/cover.jpg")
            .await
            .expect_err("mixed answers");
        assert!(matches!(error, GuardError::PrivateAddress(_)), "{error}");
    }

    #[tokio::test]
    async fn mapped_public_ipv6_is_allowed() {
        let transport = Arc::new(FakeTransport::with(&[(
            "http://[::ffff:808:808]/cover.jpg",
            ok("mapped"),
        )]));
        let fetched = guard()
            .with_transport(transport)
            .get("http://[::ffff:8.8.8.8]/cover.jpg")
            .await
            .expect("mapped public address");
        assert_eq!(fetched.body, b"mapped");
    }

    #[tokio::test]
    async fn unsupported_schemes_and_credentials_are_rejected() {
        for raw in [
            "ftp://example.com/x",
            "file:///etc/passwd",
            "data:image/png;base64,AA",
        ] {
            let error = guard().get(raw).await.expect_err("scheme must be refused");
            assert!(
                matches!(error, GuardError::UnsupportedScheme),
                "{raw}: {error}"
            );
        }
        let error = guard()
            .get("http://user:secret@example.com/x")
            .await
            .expect_err("credentials must be refused");
        assert!(matches!(error, GuardError::Credentials), "{error}");
        let error = guard().get("not a url").await.expect_err("garbage");
        assert!(matches!(error, GuardError::InvalidUrl(_)), "{error}");
    }

    #[tokio::test]
    async fn redirect_to_private_is_rejected() {
        let resolver = Arc::new(StaticResolver::new(&[
            ("public.example", "93.184.216.34"),
            ("internal.example", "10.1.50.103"),
        ]));
        let transport = Arc::new(FakeTransport::with(&[
            (
                "http://public.example/start.jpg",
                redirect("http://169.254.169.254/latest/meta-data/"),
            ),
            (
                "http://public.example/hop.jpg",
                redirect("http://internal.example/admin"),
            ),
        ]));
        let guard = guard().with_resolver(resolver).with_transport(transport);
        let error = guard
            .get("http://public.example/start.jpg")
            .await
            .expect_err("metadata redirect");
        assert!(matches!(error, GuardError::PrivateAddress(_)), "{error}");
        let error = guard
            .get("http://public.example/hop.jpg")
            .await
            .expect_err("private host redirect");
        assert!(matches!(error, GuardError::PrivateAddress(_)), "{error}");
    }

    #[tokio::test]
    async fn redirect_chain_is_limited() {
        let resolver = Arc::new(StaticResolver::new(&[("public.example", "93.184.216.34")]));
        let responses = (0..6)
            .map(|index| {
                (
                    format!("http://public.example/{index}.jpg"),
                    redirect(&format!("http://public.example/{}.jpg", index + 1)),
                )
            })
            .collect::<Vec<_>>();
        let borrowed = responses
            .iter()
            .map(|(url, response)| (url.as_str(), response.clone()))
            .collect::<Vec<_>>();
        let transport = Arc::new(FakeTransport::with(&borrowed));
        let guard = guard()
            .with_resolver(resolver)
            .with_transport(transport.clone())
            .with_limits(2, MAX_BYTES, TIMEOUT);
        let error = guard
            .get("http://public.example/0.jpg")
            .await
            .expect_err("redirect loop");
        assert!(matches!(error, GuardError::TooManyRedirects), "{error}");
        assert_eq!(
            transport.requested.lock().expect("lock").len(),
            3,
            "the initial hop plus two redirects"
        );
    }

    #[tokio::test]
    async fn redirect_to_other_scheme_is_rejected() {
        let resolver = Arc::new(StaticResolver::new(&[("public.example", "93.184.216.34")]));
        let transport = Arc::new(FakeTransport::with(&[(
            "http://public.example/start.jpg",
            redirect("file:///etc/passwd"),
        )]));
        let error = guard()
            .with_resolver(resolver)
            .with_transport(transport)
            .get("http://public.example/start.jpg")
            .await
            .expect_err("file redirect");
        assert!(matches!(error, GuardError::UnsupportedScheme), "{error}");
    }

    #[tokio::test]
    async fn redirect_without_location_is_rejected() {
        let resolver = Arc::new(StaticResolver::new(&[("public.example", "93.184.216.34")]));
        let transport = Arc::new(FakeTransport::with(&[(
            "http://public.example/start.jpg",
            RawResponse {
                status: 302,
                location: None,
                content_type: None,
                body: Vec::new(),
            },
        )]));
        let error = guard()
            .with_resolver(resolver)
            .with_transport(transport)
            .get("http://public.example/start.jpg")
            .await
            .expect_err("location missing");
        assert!(
            matches!(error, GuardError::RedirectWithoutLocation),
            "{error}"
        );
    }

    #[tokio::test]
    async fn relative_redirects_are_followed() {
        let resolver = Arc::new(StaticResolver::new(&[("public.example", "93.184.216.34")]));
        let transport = Arc::new(FakeTransport::with(&[
            (
                "http://public.example/a/start.jpg",
                redirect("/b/cover.jpg"),
            ),
            ("http://public.example/b/cover.jpg", ok("final")),
        ]));
        let fetched = guard()
            .with_resolver(resolver)
            .with_transport(transport.clone())
            .get("http://public.example/a/start.jpg")
            .await
            .expect("relative redirect");
        assert_eq!(fetched.body, b"final");
        assert_eq!(fetched.url.as_str(), "http://public.example/b/cover.jpg");
        assert_eq!(
            transport.requested.lock().expect("lock").len(),
            2,
            "one request per hop"
        );
    }

    #[tokio::test]
    async fn oversized_bodies_are_rejected() {
        let resolver = Arc::new(StaticResolver::new(&[("public.example", "93.184.216.34")]));
        let transport = Arc::new(FakeTransport::with(&[(
            "http://public.example/big.jpg",
            ok("0123456789"),
        )]));
        let error = guard()
            .with_resolver(resolver)
            .with_transport(transport)
            .with_limits(MAX_REDIRECTS, 4, TIMEOUT)
            .get("http://public.example/big.jpg")
            .await
            .expect_err("body too large");
        assert!(matches!(error, GuardError::TooLarge(4)), "{error}");
    }

    #[tokio::test]
    async fn dns_is_pinned_to_the_validated_address() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = vec![0_u8; 1024];
            let read = socket.read(&mut buffer).await.expect("read request");
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi")
                .await
                .expect("write response");
            let _ = socket.shutdown().await;
            request
        });
        let resolver = Arc::new(StaticResolver::new(&[("pinned.example", "127.0.0.1")]));
        // Private targets are allowed here on purpose: this test proves the
        // resolver answer is used and the Host header keeps the name.
        let fetched = UrlGuard::new(true)
            .with_resolver(resolver)
            .get(&format!(
                "http://pinned.example:{}/cover.jpg",
                address.port()
            ))
            .await
            .expect("pinned fetch");
        assert_eq!(fetched.body, b"hi");
        let request = server.await.expect("server task");
        assert!(request.starts_with("GET /cover.jpg HTTP/1.1"), "{request}");
        assert!(
            request.contains(&format!("host: pinned.example:{}", address.port()))
                || request.contains(&format!("Host: pinned.example:{}", address.port())),
            "Host header must keep the validated name: {request}"
        );
    }

    #[tokio::test]
    async fn errors_do_not_leak_the_full_url() {
        let error = guard()
            .get("http://169.254.169.254/latest/meta-data?token=s3cr3t")
            .await
            .expect_err("metadata address");
        let message = format!("{error}");
        assert!(!message.contains("s3cr3t"), "{message}");
        assert!(!message.contains("meta-data"), "{message}");
        assert!(message.contains("private address"), "{message}");
    }

    #[test]
    fn private_ranges_are_classified() {
        for address in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.0.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.7",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12:3456:789a::1",
            "fe80::1",
            "fec0::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            let ip: IpAddr = address.parse().expect("ip");
            assert!(is_private_ip(ip), "{address} must be private");
        }
        for address in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            let ip: IpAddr = address.parse().expect("ip");
            assert!(!is_private_ip(ip), "{address} must be public");
        }
    }
}
