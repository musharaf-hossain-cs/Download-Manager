//! Durable exact-host overrides for connection, request-header, segmentation, and proxy policy.

use std::{collections::HashSet, time::Duration};

use reqwest::{
    blocking::{Client, ClientBuilder},
    header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT},
    redirect::Policy,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::{Host, Url};

use crate::{
    credentials::{CredentialError, CredentialRef},
    domain::HttpUrl,
    proxy::{
        ProxyConfig, ProxyError, configure_proxy_builder, configure_proxy_builder_with_credentials,
    },
};

const MAX_CONNECTIONS: u8 = 32;
const MAX_HEADERS: usize = 32;
const MAX_HEADER_VALUE_LENGTH: usize = 4_096;
const MAX_HEADER_BYTES: usize = 16 * 1_024;
const MAX_USER_AGENT_LENGTH: usize = 512;
const MAX_NO_PROXY_RULES: usize = 128;
type OptionalPasswordLoader<'a> =
    Option<&'a dyn Fn(&CredentialRef) -> Result<String, CredentialError>>;

#[derive(Debug, Error)]
pub enum ServerRuleError {
    #[error("server rule host must be one valid hostname or IP address")]
    InvalidHost,
    #[error("server connection cap must be between 1 and 32")]
    InvalidConnectionCap,
    #[error("server rule user-agent must contain 1 to 512 valid header characters")]
    InvalidUserAgent,
    #[error("server rule has too many header overrides")]
    TooManyHeaders,
    #[error("server rule header name is invalid or duplicated: {0}")]
    InvalidHeaderName(String),
    #[error("server rule header value is invalid or oversized: {0}")]
    InvalidHeaderValue(String),
    #[error("server rule header is managed by the engine and cannot be overridden: {0}")]
    ManagedHeader(String),
    #[error("server rule header overrides exceed the aggregate size limit")]
    HeadersTooLarge,
    #[error("server rule proxy URLs cannot contain credentials, queries, or fragments")]
    UnsafeProxyUrl,
    #[error("server rule no-proxy list is invalid or oversized")]
    InvalidNoProxy,
    #[error("request URL is missing a host")]
    MissingRequestHost,
    #[error("server-rule HTTP client configuration failed: {0}")]
    Client(#[from] ProxyError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderOverride {
    name: String,
    value: String,
}

impl HeaderOverride {
    /// Creates one non-sensitive, end-to-end request header override.
    ///
    /// # Errors
    /// Returns an error for malformed, duplicated-by-policy, secret-bearing, framing, routing,
    /// range, or user-agent headers. User-agent has a dedicated rule field.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Result<Self, ServerRuleError> {
        let header = Self {
            name: name.into().to_ascii_lowercase(),
            value: value.into(),
        };
        validate_header(&header)?;
        Ok(header)
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRule {
    host: String,
    connection_cap: Option<u8>,
    user_agent: Option<String>,
    headers: Vec<HeaderOverride>,
    disable_segmentation: bool,
    proxy_override: Option<ProxyConfig>,
}

impl ServerRule {
    /// Creates and validates an exact-host server rule.
    ///
    /// # Errors
    /// Returns an error for a URL/pattern in place of a host, invalid connection/header values,
    /// or a proxy override that would persist credentials.
    pub fn new(
        host: impl Into<String>,
        connection_cap: Option<u8>,
        user_agent: Option<String>,
        headers: Vec<HeaderOverride>,
        disable_segmentation: bool,
        proxy_override: Option<ProxyConfig>,
    ) -> Result<Self, ServerRuleError> {
        let host = normalize_host(&host.into())?;
        if connection_cap.is_some_and(|cap| !(1..=MAX_CONNECTIONS).contains(&cap)) {
            return Err(ServerRuleError::InvalidConnectionCap);
        }
        validate_user_agent(user_agent.as_deref())?;
        validate_headers(&headers)?;
        if let Some(proxy) = proxy_override.as_ref() {
            validate_proxy(proxy)?;
        }
        Ok(Self {
            host,
            connection_cap,
            user_agent,
            headers,
            disable_segmentation,
            proxy_override,
        })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub const fn connection_cap(&self) -> Option<u8> {
        self.connection_cap
    }

    #[must_use]
    pub fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }

    #[must_use]
    pub fn headers(&self) -> &[HeaderOverride] {
        &self.headers
    }

    #[must_use]
    pub const fn disables_segmentation(&self) -> bool {
        self.disable_segmentation
    }

    #[must_use]
    pub const fn proxy_override(&self) -> Option<&ProxyConfig> {
        self.proxy_override.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRequestDefaults {
    pub connection_cap: u8,
    pub segmentation_enabled: bool,
    pub user_agent: Option<String>,
    pub proxy: ProxyConfig,
}

impl ServerRequestDefaults {
    /// Validates the global values used when a server rule omits an override.
    ///
    /// # Errors
    /// Returns an error for an invalid connection cap or default user-agent.
    pub fn validate(&self) -> Result<(), ServerRuleError> {
        if !(1..=MAX_CONNECTIONS).contains(&self.connection_cap) {
            return Err(ServerRuleError::InvalidConnectionCap);
        }
        validate_user_agent(self.user_agent.as_deref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    SingleStream,
    Segmented { connection_cap: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRequestPlan {
    host: String,
    connection_cap: u8,
    user_agent: Option<String>,
    headers: Vec<HeaderOverride>,
    transfer_mode: TransferMode,
    proxy: ProxyConfig,
    matched_rule: bool,
}

impl ServerRequestPlan {
    /// Resolves global defaults and an optional exact-host override into one immutable plan.
    ///
    /// # Errors
    /// Returns an error when the defaults are invalid or the request URL has no host.
    pub fn resolve(
        url: &HttpUrl,
        defaults: &ServerRequestDefaults,
        rule: Option<&ServerRule>,
    ) -> Result<Self, ServerRuleError> {
        defaults.validate()?;
        let host = request_host(url)?;
        let matching_rule = rule.filter(|candidate| candidate.host == host);
        let connection_cap = matching_rule
            .and_then(ServerRule::connection_cap)
            .unwrap_or(defaults.connection_cap);
        let segmentation_enabled = defaults.segmentation_enabled
            && !matching_rule.is_some_and(ServerRule::disables_segmentation)
            && connection_cap > 1;
        Ok(Self {
            host,
            connection_cap,
            user_agent: matching_rule.and_then(ServerRule::user_agent).map_or_else(
                || defaults.user_agent.clone(),
                |value| Some(value.to_owned()),
            ),
            headers: matching_rule.map_or_else(Vec::new, |value| value.headers.clone()),
            transfer_mode: if segmentation_enabled {
                TransferMode::Segmented { connection_cap }
            } else {
                TransferMode::SingleStream
            },
            proxy: matching_rule
                .and_then(ServerRule::proxy_override)
                .cloned()
                .unwrap_or_else(|| defaults.proxy.clone()),
            matched_rule: matching_rule.is_some(),
        })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub const fn connection_cap(&self) -> u8 {
        self.connection_cap
    }

    #[must_use]
    pub const fn transfer_mode(&self) -> TransferMode {
        self.transfer_mode
    }

    #[must_use]
    pub const fn proxy(&self) -> &ProxyConfig {
        &self.proxy
    }

    #[must_use]
    pub const fn matched_rule(&self) -> bool {
        self.matched_rule
    }

    /// Builds the bounded HTTP client used by both single-stream and segmented transfer paths.
    ///
    /// # Errors
    /// Returns an error if the selected proxy or default headers cannot be configured.
    pub fn build_client(&self) -> Result<Client, ServerRuleError> {
        self.build_client_inner(None)
    }

    /// Builds the bounded HTTP client while resolving proxy authentication only through a
    /// current-user credential-vault boundary.
    ///
    /// # Errors
    /// Returns an error if the credential cannot be loaded or the selected proxy/default headers
    /// cannot be configured.
    pub fn build_client_with_credentials(
        &self,
        load_password: &dyn Fn(&CredentialRef) -> Result<String, CredentialError>,
    ) -> Result<Client, ServerRuleError> {
        self.build_client_inner(Some(load_password))
    }

    fn build_client_inner(
        &self,
        load_password: OptionalPasswordLoader<'_>,
    ) -> Result<Client, ServerRuleError> {
        let mut headers = HeaderMap::new();
        for header in &self.headers {
            let name = HeaderName::from_bytes(header.name.as_bytes())
                .map_err(|_| ServerRuleError::InvalidHeaderName(header.name.clone()))?;
            let value = HeaderValue::from_str(&header.value)
                .map_err(|_| ServerRuleError::InvalidHeaderValue(header.name.clone()))?;
            headers.insert(name, value);
        }
        if let Some(user_agent) = self.user_agent.as_deref() {
            headers.insert(
                USER_AGENT,
                HeaderValue::from_str(user_agent).map_err(|_| ServerRuleError::InvalidUserAgent)?,
            );
        }
        let builder = ClientBuilder::new()
            .redirect(Policy::limited(10))
            .connect_timeout(Duration::from_secs(30))
            .default_headers(headers);
        let builder = if let Some(load_password) = load_password {
            configure_proxy_builder_with_credentials(builder, &self.proxy, load_password)?
        } else {
            configure_proxy_builder(builder, &self.proxy)?
        };
        Ok(builder.build().map_err(ProxyError::from)?)
    }
}

pub(crate) fn request_host(url: &HttpUrl) -> Result<String, ServerRuleError> {
    let parsed = Url::parse(url.as_str()).map_err(|_| ServerRuleError::MissingRequestHost)?;
    parsed
        .host_str()
        .map(str::to_owned)
        .ok_or(ServerRuleError::MissingRequestHost)
}

fn normalize_host(value: &str) -> Result<String, ServerRuleError> {
    if value.trim() != value || value.is_empty() || value.len() > 253 || value.contains('/') {
        return Err(ServerRuleError::InvalidHost);
    }
    Host::parse(value)
        .map(|host| host.to_string().to_ascii_lowercase())
        .map_err(|_| ServerRuleError::InvalidHost)
}

fn validate_headers(headers: &[HeaderOverride]) -> Result<(), ServerRuleError> {
    if headers.len() > MAX_HEADERS {
        return Err(ServerRuleError::TooManyHeaders);
    }
    let mut names = HashSet::with_capacity(headers.len());
    let mut total = 0_usize;
    for header in headers {
        validate_header(header)?;
        if !names.insert(header.name.as_str()) {
            return Err(ServerRuleError::InvalidHeaderName(header.name.clone()));
        }
        total = total.saturating_add(header.name.len() + header.value.len());
    }
    if total > MAX_HEADER_BYTES {
        return Err(ServerRuleError::HeadersTooLarge);
    }
    Ok(())
}

fn validate_user_agent(value: Option<&str>) -> Result<(), ServerRuleError> {
    if value.is_some_and(|value| {
        value.trim().is_empty()
            || value.len() > MAX_USER_AGENT_LENGTH
            || HeaderValue::from_str(value).is_err()
    }) {
        Err(ServerRuleError::InvalidUserAgent)
    } else {
        Ok(())
    }
}

fn validate_header(header: &HeaderOverride) -> Result<(), ServerRuleError> {
    let name = HeaderName::from_bytes(header.name.as_bytes())
        .map_err(|_| ServerRuleError::InvalidHeaderName(header.name.clone()))?;
    if managed_header(&name) {
        return Err(ServerRuleError::ManagedHeader(header.name.clone()));
    }
    if header.value.len() > MAX_HEADER_VALUE_LENGTH || HeaderValue::from_str(&header.value).is_err()
    {
        return Err(ServerRuleError::InvalidHeaderValue(header.name.clone()));
    }
    Ok(())
}

fn managed_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "range"
            | "user-agent"
    )
}

fn validate_proxy(proxy: &ProxyConfig) -> Result<(), ServerRuleError> {
    let no_proxy = match proxy {
        ProxyConfig::Manual(manual) => {
            for value in [manual.http_proxy.as_ref(), manual.https_proxy.as_ref()]
                .into_iter()
                .flatten()
            {
                let parsed =
                    Url::parse(value.as_str()).map_err(|_| ServerRuleError::UnsafeProxyUrl)?;
                if !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.query().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(ServerRuleError::UnsafeProxyUrl);
                }
            }
            &manual.no_proxy
        }
        ProxyConfig::Socks(socks) => &socks.no_proxy,
        ProxyConfig::System | ProxyConfig::Direct => return Ok(()),
    };
    if no_proxy.len() > MAX_NO_PROXY_RULES
        || no_proxy.iter().any(|rule| {
            rule.trim() != rule
                || rule.is_empty()
                || rule.len() > 253
                || rule.bytes().any(|byte| byte.is_ascii_control())
        })
    {
        return Err(ServerRuleError::InvalidNoProxy);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        HeaderOverride, ServerRequestDefaults, ServerRequestPlan, ServerRule, TransferMode,
    };
    use crate::{
        domain::HttpUrl,
        proxy::{ManualProxyConfig, ProxyConfig},
        storage::Database,
        test_support::{Fixture, FixtureServer},
    };

    fn defaults() -> ServerRequestDefaults {
        ServerRequestDefaults {
            connection_cap: 8,
            segmentation_enabled: true,
            user_agent: Some("DownloadManager/1.0".to_owned()),
            proxy: ProxyConfig::System,
        }
    }

    #[test]
    fn durable_exact_host_rule_survives_restart_and_drives_the_request_plan() {
        let directory = tempdir().expect("directory");
        let path = directory.path().join("rules.sqlite");
        let rule = ServerRule::new(
            "EXAMPLE.TEST",
            Some(3),
            Some("RuleAgent/2.0".to_owned()),
            vec![HeaderOverride::new("X-Site-Profile", "archive").expect("header")],
            true,
            Some(ProxyConfig::Direct),
        )
        .expect("rule");
        let mut database = Database::open(&path).expect("database");
        database.upsert_server_rule(&rule).expect("save rule");
        drop(database);

        let mut database = Database::open(&path).expect("reopen database");
        let url = HttpUrl::parse("https://example.test/file.bin").expect("URL");
        let stored = database
            .server_rule_for_url(&url)
            .expect("lookup")
            .expect("stored rule");
        assert_eq!(stored, rule);
        let plan = ServerRequestPlan::resolve(&url, &defaults(), Some(&stored)).expect("plan");
        assert_eq!(plan.connection_cap(), 3);
        assert_eq!(plan.transfer_mode(), TransferMode::SingleStream);
        assert_eq!(plan.proxy(), &ProxyConfig::Direct);
        assert!(plan.matched_rule());

        let other = HttpUrl::parse("https://cdn.example.test/file.bin").expect("URL");
        assert!(
            database
                .server_rule_for_url(&other)
                .expect("lookup")
                .is_none()
        );
        let inherited = ServerRequestPlan::resolve(&other, &defaults(), None).expect("plan");
        assert_eq!(
            inherited.transfer_mode(),
            TransferMode::Segmented { connection_cap: 8 }
        );
        assert!(!inherited.matched_rule());
        assert_eq!(
            database.list_server_rules().expect("list rules"),
            vec![rule]
        );
        assert!(database.delete_server_rule("EXAMPLE.TEST").expect("delete"));
        assert!(
            database
                .server_rule_for_url(&url)
                .expect("lookup")
                .is_none()
        );
    }

    #[test]
    fn resolved_client_applies_user_agent_headers_and_direct_proxy_override() {
        let server = FixtureServer::start(vec![Fixture::ok("ok")]);
        let url = HttpUrl::parse(server.url("/rule")).expect("URL");
        let host = url::Url::parse(url.as_str())
            .expect("parsed URL")
            .host_str()
            .expect("host")
            .to_owned();
        let rule = ServerRule::new(
            host,
            Some(2),
            Some("RuleAgent/2.0".to_owned()),
            vec![HeaderOverride::new("X-Site-Profile", "archive").expect("header")],
            false,
            Some(ProxyConfig::Direct),
        )
        .expect("rule");
        let plan = ServerRequestPlan::resolve(&url, &defaults(), Some(&rule)).expect("plan");
        let body = plan
            .build_client()
            .expect("client")
            .get(url.as_str())
            .send()
            .expect("request")
            .text()
            .expect("body");
        assert_eq!(body, "ok");
        let requests = server.requests();
        assert_eq!(requests[0].header("user-agent"), Some("RuleAgent/2.0"));
        assert_eq!(requests[0].header("x-site-profile"), Some("archive"));
    }

    #[test]
    fn rules_reject_injection_secrets_unsafe_proxies_and_invalid_caps() {
        assert!(HeaderOverride::new("X-Test", "good\r\nInjected: bad").is_err());
        assert!(HeaderOverride::new("Authorization", "Bearer secret").is_err());
        assert!(
            ServerRule::new("https://example.test", Some(2), None, vec![], false, None).is_err()
        );
        assert!(ServerRule::new("example.test", Some(0), None, vec![], false, None).is_err());
        let ipv6 = ServerRule::new("[::1]", Some(1), None, vec![], false, None).expect("IPv6 rule");
        let ipv6_url = HttpUrl::parse("http://[::1]/file").expect("IPv6 URL");
        assert_eq!(
            ServerRequestPlan::resolve(&ipv6_url, &defaults(), Some(&ipv6))
                .expect("IPv6 plan")
                .connection_cap(),
            1
        );
        let unsafe_proxy = ProxyConfig::Manual(ManualProxyConfig {
            http_proxy: Some(
                HttpUrl::parse("http://user:password@proxy.test:8080").expect("proxy URL"),
            ),
            https_proxy: None,
            authentication: None,
            no_proxy: vec![],
        });
        assert!(
            ServerRule::new(
                "example.test",
                Some(2),
                None,
                vec![],
                false,
                Some(unsafe_proxy)
            )
            .is_err()
        );
    }
}
