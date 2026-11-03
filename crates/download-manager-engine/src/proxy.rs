//! HTTP(S) and SOCKS proxy client configuration with vault-backed authentication.

use reqwest::{
    Proxy,
    blocking::{Client, ClientBuilder},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::{
    credentials::{CredentialError, CredentialRef},
    domain::HttpUrl,
};

const MAX_PROXY_URL_LENGTH: usize = 2_048;
const MAX_PROXY_USERNAME_LENGTH: usize = 512;
const MAX_PROXY_PASSWORD_LENGTH: usize = 1_024;
type OptionalPasswordLoader<'a> =
    Option<&'a dyn Fn(&CredentialRef) -> Result<String, CredentialError>>;

/// A validated, non-secret SOCKS endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SocksProxyUrl(String);

impl SocksProxyUrl {
    /// Parses a SOCKS4, `SOCKS4a`, SOCKS5, or `SOCKS5h` endpoint.
    ///
    /// Credentials, paths, queries, and fragments are forbidden so secrets never enter durable
    /// configuration. Authentication uses [`ProxyAuthentication`] instead.
    ///
    /// # Errors
    /// Returns [`ProxyError::InvalidConfiguration`] for an unsafe or unsupported endpoint.
    pub fn parse(value: impl Into<String>) -> Result<Self, ProxyError> {
        let value = value.into();
        if value.len() > MAX_PROXY_URL_LENGTH {
            return Err(ProxyError::InvalidConfiguration);
        }
        let parsed = Url::parse(&value).map_err(|_| ProxyError::InvalidConfiguration)?;
        if !matches!(parsed.scheme(), "socks4" | "socks4a" | "socks5" | "socks5h")
            || parsed.host().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !matches!(parsed.path(), "" | "/")
        {
            return Err(ProxyError::InvalidConfiguration);
        }
        Ok(Self(parsed.into()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SocksProxyUrl {
    type Error = ProxyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<SocksProxyUrl> for String {
    fn from(value: SocksProxyUrl) -> Self {
        value.0
    }
}

/// Non-secret proxy login metadata. The password remains in the current-user credential vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyAuthentication {
    pub username: String,
    pub password: CredentialRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualProxyConfig {
    pub http_proxy: Option<HttpUrl>,
    pub https_proxy: Option<HttpUrl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<ProxyAuthentication>,
    pub no_proxy: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SocksProxyConfig {
    pub proxy_url: SocksProxyUrl,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<ProxyAuthentication>,
    pub no_proxy: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", content = "config")]
pub enum ProxyConfig {
    /// Uses the platform/environment proxy discovery supported by the HTTP stack.
    System,
    /// Makes direct connections and ignores system proxy configuration.
    Direct,
    Manual(ManualProxyConfig),
    Socks(SocksProxyConfig),
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("proxy client configuration failed: {0}")]
    Client(#[from] reqwest::Error),
    #[error("proxy configuration is invalid or contains inline credentials")]
    InvalidConfiguration,
    #[error("authenticated proxy requires a current-user credential resolver")]
    CredentialsRequired,
    #[error("proxy credential could not be loaded from the current-user vault: {0}")]
    Credential(#[from] CredentialError),
    #[error("SOCKS4 authentication is not supported; use SOCKS5 username/password authentication")]
    Socks4AuthenticationUnsupported,
}

/// Builds an HTTP client for system, direct, or per-scheme manual proxy routing.
///
/// # Errors
/// Returns an error when the underlying HTTP client cannot be configured.
pub fn build_proxy_client(config: &ProxyConfig) -> Result<Client, ProxyError> {
    Ok(configure_proxy_builder_inner(Client::builder(), config, None)?.build()?)
}

/// Builds a proxy client, resolving an optional password only from a user-bound secret store.
///
/// # Errors
/// Returns an error when the credential cannot be loaded or the client cannot be configured.
pub fn build_proxy_client_with_credentials(
    config: &ProxyConfig,
    load_password: &dyn Fn(&CredentialRef) -> Result<String, CredentialError>,
) -> Result<Client, ProxyError> {
    Ok(configure_proxy_builder_inner(Client::builder(), config, Some(load_password))?.build()?)
}

/// Applies system, direct, or per-scheme manual proxy routing to an existing client builder.
///
/// This lets request policies add timeouts and default headers without bypassing the selected
/// proxy route.
///
/// # Errors
/// Returns an error when the underlying proxy configuration cannot be attached.
pub fn configure_proxy_builder(
    builder: ClientBuilder,
    config: &ProxyConfig,
) -> Result<ClientBuilder, ProxyError> {
    configure_proxy_builder_inner(builder, config, None)
}

/// Applies proxy routing while resolving authentication through a current-user secret store.
///
/// # Errors
/// Returns an error when the password cannot be loaded or the proxy configuration is invalid.
pub fn configure_proxy_builder_with_credentials(
    builder: ClientBuilder,
    config: &ProxyConfig,
    load_password: &dyn Fn(&CredentialRef) -> Result<String, CredentialError>,
) -> Result<ClientBuilder, ProxyError> {
    configure_proxy_builder_inner(builder, config, Some(load_password))
}

fn configure_proxy_builder_inner(
    builder: ClientBuilder,
    config: &ProxyConfig,
    load_password: OptionalPasswordLoader<'_>,
) -> Result<ClientBuilder, ProxyError> {
    match config {
        ProxyConfig::System => Ok(builder),
        ProxyConfig::Direct => Ok(builder.no_proxy()),
        ProxyConfig::Manual(manual) => {
            validate_authentication(manual.authentication.as_ref())?;
            let web_proxy = manual
                .http_proxy
                .as_ref()
                .map(|value| safe_http_proxy_url(value.as_str()))
                .transpose()?;
            let secure_proxy = manual
                .https_proxy
                .as_ref()
                .map(|value| safe_http_proxy_url(value.as_str()))
                .transpose()?;
            let no_proxy = manual.no_proxy.clone();
            let mut proxy = Proxy::custom(move |target| {
                if should_bypass_proxy(target, &no_proxy) {
                    return None;
                }
                match target.scheme() {
                    "http" => web_proxy.clone(),
                    "https" => secure_proxy.clone(),
                    _ => None,
                }
            });
            if let Some(authentication) = manual.authentication.as_ref() {
                let password = resolve_password(authentication, load_password)?;
                proxy = proxy.basic_auth(&authentication.username, &password);
            }
            Ok(builder.no_proxy().proxy(proxy))
        }
        ProxyConfig::Socks(socks) => {
            validate_authentication(socks.authentication.as_ref())?;
            let mut proxy_url = Url::parse(socks.proxy_url.as_str())
                .map_err(|_| ProxyError::InvalidConfiguration)?;
            if let Some(authentication) = socks.authentication.as_ref() {
                if matches!(proxy_url.scheme(), "socks4" | "socks4a") {
                    return Err(ProxyError::Socks4AuthenticationUnsupported);
                }
                let password = resolve_password(authentication, load_password)?;
                if authentication.username.len() > u8::MAX.into() || password.len() > u8::MAX.into()
                {
                    return Err(ProxyError::InvalidConfiguration);
                }
                proxy_url
                    .set_username(&authentication.username)
                    .map_err(|()| ProxyError::InvalidConfiguration)?;
                proxy_url
                    .set_password(Some(&password))
                    .map_err(|()| ProxyError::InvalidConfiguration)?;
            }
            let no_proxy = socks.no_proxy.clone();
            let proxy = Proxy::custom(move |target| {
                (!should_bypass_proxy(target, &no_proxy)).then(|| proxy_url.clone())
            });
            Ok(builder.no_proxy().proxy(proxy))
        }
    }
}

fn safe_http_proxy_url(value: &str) -> Result<Url, ProxyError> {
    let parsed = Url::parse(value).map_err(|_| ProxyError::InvalidConfiguration)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ProxyError::InvalidConfiguration);
    }
    Ok(parsed)
}

fn validate_authentication(authentication: Option<&ProxyAuthentication>) -> Result<(), ProxyError> {
    if authentication.is_some_and(|value| {
        value.username.trim().is_empty()
            || value.username.len() > MAX_PROXY_USERNAME_LENGTH
            || value.username.bytes().any(|byte| byte.is_ascii_control())
            || value.password.service.trim().is_empty()
            || value.password.service.len() > 1_024
            || value.password.account.trim().is_empty()
            || value.password.account.len() > 512
    }) {
        Err(ProxyError::InvalidConfiguration)
    } else {
        Ok(())
    }
}

fn resolve_password(
    authentication: &ProxyAuthentication,
    load_password: OptionalPasswordLoader<'_>,
) -> Result<String, ProxyError> {
    let password = load_password.ok_or(ProxyError::CredentialsRequired)?(&authentication.password)?;
    if password.is_empty() || password.len() > MAX_PROXY_PASSWORD_LENGTH {
        return Err(ProxyError::InvalidConfiguration);
    }
    Ok(password)
}

/// Returns whether a URL host matches an ordered no-proxy rule list.
#[must_use]
pub fn should_bypass_proxy(target: &Url, rules: &[String]) -> bool {
    let Some(host) = target.host_str() else {
        return false;
    };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    rules.iter().any(|rule| {
        let rule = rule.trim().to_ascii_lowercase();
        if rule == "*" {
            return true;
        }
        if rule == "<local>" {
            return !host.contains('.');
        }
        let rule = rule
            .strip_prefix("*.")
            .or_else(|| rule.strip_prefix('.'))
            .unwrap_or(&rule)
            .trim_matches(['[', ']']);
        host == rule
            || host
                .strip_suffix(rule)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use url::Url;

    use super::{
        ManualProxyConfig, ProxyAuthentication, ProxyConfig, ProxyError, SocksProxyConfig,
        SocksProxyUrl, build_proxy_client, build_proxy_client_with_credentials,
        should_bypass_proxy,
    };
    use crate::{
        credentials::CredentialRef,
        domain::HttpUrl,
        test_support::{Fixture, FixtureServer},
    };

    #[test]
    fn no_proxy_wildcard_matrix_is_host_scoped() {
        let cases = [
            ("https://example.test/a", vec!["example.test"], true),
            ("https://api.example.test/a", vec!["*.example.test"], true),
            ("https://badexample.test/a", vec![".example.test"], false),
            ("http://localhost/a", vec!["<local>"], true),
            ("http://127.0.0.1/a", vec!["127.0.0.1"], true),
            ("https://elsewhere.test/a", vec!["*"], true),
            ("https://elsewhere.test/a", vec!["example.test"], false),
        ];
        for (url, rules, expected) in cases {
            let rules = rules.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(
                should_bypass_proxy(&Url::parse(url).expect("URL"), &rules),
                expected,
                "{url}"
            );
        }
    }

    #[test]
    fn manual_proxy_routes_http_and_no_proxy_rule_bypasses_it() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("proxy bind");
        let proxy_address = listener.local_addr().expect("proxy address");
        let proxy = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("proxy accept");
            let request = read_headers(&mut stream);
            assert!(request.starts_with("GET http://origin.invalid/file HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nthrough-proxy",
                )
                .expect("proxy response");
        });
        let manual = ProxyConfig::Manual(ManualProxyConfig {
            http_proxy: Some(
                HttpUrl::parse(format!("http://{proxy_address}")).expect("manual proxy URL"),
            ),
            https_proxy: None,
            authentication: None,
            no_proxy: Vec::new(),
        });
        let body = build_proxy_client(&manual)
            .expect("client")
            .get("http://origin.invalid/file")
            .send()
            .expect("proxied request")
            .text()
            .expect("proxy body");
        assert_eq!(body, "through-proxy");
        proxy.join().expect("proxy");

        let origin = FixtureServer::start(vec![Fixture::ok("direct")]);
        let bypass = ProxyConfig::Manual(ManualProxyConfig {
            http_proxy: Some(HttpUrl::parse("http://127.0.0.1:1").expect("dead proxy")),
            https_proxy: None,
            authentication: None,
            no_proxy: vec!["127.0.0.1".to_owned()],
        });
        let direct = build_proxy_client(&bypass)
            .expect("client")
            .get(origin.url("/direct"))
            .send()
            .expect("direct request")
            .text()
            .expect("direct body");
        assert_eq!(direct, "direct");
    }

    #[test]
    fn socks4_routes_http_through_the_configured_tunnel() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("SOCKS4 bind");
        let proxy_address = listener.local_addr().expect("SOCKS4 address");
        let proxy = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("SOCKS4 accept");
            let mut request = [0_u8; 8];
            stream.read_exact(&mut request).expect("SOCKS4 request");
            assert_eq!(&request[..2], &[4, 1]);
            assert_eq!(&request[4..8], &[127, 0, 0, 1]);
            assert_eq!(read_nul_terminated(&mut stream), Vec::<u8>::new());
            stream
                .write_all(&[0, 0x5a, 0, 0, 0, 0, 0, 0])
                .expect("SOCKS4 grant");
            assert!(read_headers(&mut stream).starts_with("GET /file HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nsocks4-route",
                )
                .expect("tunnel response");
        });
        let config = ProxyConfig::Socks(SocksProxyConfig {
            proxy_url: SocksProxyUrl::parse(format!("socks4://{proxy_address}"))
                .expect("SOCKS4 URL"),
            authentication: None,
            no_proxy: Vec::new(),
        });
        let body = build_proxy_client(&config)
            .expect("SOCKS4 client")
            .get("http://127.0.0.1:9/file")
            .send()
            .expect("SOCKS4 request")
            .text()
            .expect("SOCKS4 body");
        assert_eq!(body, "socks4-route");
        proxy.join().expect("SOCKS4 proxy");
    }

    #[test]
    fn socks5_authentication_is_loaded_by_reference_and_never_serialized() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("SOCKS5 bind");
        let proxy_address = listener.local_addr().expect("SOCKS5 address");
        let proxy = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("SOCKS5 accept");
            let mut greeting = [0_u8; 2];
            stream.read_exact(&mut greeting).expect("SOCKS5 greeting");
            assert_eq!(greeting[0], 5);
            let mut methods = vec![0_u8; usize::from(greeting[1])];
            stream.read_exact(&mut methods).expect("SOCKS5 methods");
            assert!(methods.contains(&2));
            stream.write_all(&[5, 2]).expect("select auth");

            let mut auth = [0_u8; 2];
            stream.read_exact(&mut auth).expect("auth header");
            assert_eq!(auth[0], 1);
            let mut username = vec![0_u8; usize::from(auth[1])];
            stream.read_exact(&mut username).expect("username");
            let mut password_length = [0_u8; 1];
            stream
                .read_exact(&mut password_length)
                .expect("password length");
            let mut password = vec![0_u8; usize::from(password_length[0])];
            stream.read_exact(&mut password).expect("password");
            assert_eq!(username, b"proxy-user");
            assert_eq!(password, b"vault-secret");
            stream.write_all(&[1, 0]).expect("auth success");

            consume_socks5_connect(&mut stream);
            stream
                .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
                .expect("connect success");
            assert!(read_headers(&mut stream).starts_with("GET /file HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nsocks5-route",
                )
                .expect("tunnel response");
        });
        let reference = CredentialRef {
            service: "DownloadManager/proxy/127.0.0.1".to_owned(),
            account: "proxy-user".to_owned(),
        };
        let config = ProxyConfig::Socks(SocksProxyConfig {
            proxy_url: SocksProxyUrl::parse(format!("socks5://{proxy_address}"))
                .expect("SOCKS5 URL"),
            authentication: Some(ProxyAuthentication {
                username: "proxy-user".to_owned(),
                password: reference.clone(),
            }),
            no_proxy: Vec::new(),
        });
        let encoded = serde_json::to_string(&config).expect("serialize config");
        assert!(!encoded.contains("vault-secret"));
        assert!(build_proxy_client(&config).is_err());
        let body = build_proxy_client_with_credentials(&config, &|requested| {
            assert_eq!(requested, &reference);
            Ok("vault-secret".to_owned())
        })
        .expect("SOCKS5 client")
        .get("http://127.0.0.1:9/file")
        .send()
        .expect("SOCKS5 request")
        .text()
        .expect("SOCKS5 body");
        assert_eq!(body, "socks5-route");
        proxy.join().expect("SOCKS5 proxy");
    }

    #[test]
    fn authenticated_http_proxy_uses_the_same_vault_boundary() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("proxy bind");
        let proxy_address = listener.local_addr().expect("proxy address");
        let proxy = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("proxy accept");
            let request = read_headers(&mut stream);
            assert!(
                request.contains("proxy-authorization: Basic cHJveHktdXNlcjp2YXVsdC1zZWNyZXQ=")
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nauth")
                .expect("proxy response");
        });
        let config = ProxyConfig::Manual(ManualProxyConfig {
            http_proxy: Some(HttpUrl::parse(format!("http://{proxy_address}")).expect("proxy URL")),
            https_proxy: None,
            authentication: Some(ProxyAuthentication {
                username: "proxy-user".to_owned(),
                password: CredentialRef {
                    service: "DownloadManager/proxy/http".to_owned(),
                    account: "proxy-user".to_owned(),
                },
            }),
            no_proxy: Vec::new(),
        });
        for invalid_password in [String::new(), "x".repeat(1_025)] {
            assert!(matches!(
                build_proxy_client_with_credentials(&config, &|_| Ok(invalid_password.clone())),
                Err(ProxyError::InvalidConfiguration)
            ));
        }
        let body = build_proxy_client_with_credentials(&config, &|_| Ok("vault-secret".to_owned()))
            .expect("authenticated proxy client")
            .get("http://origin.invalid/file")
            .send()
            .expect("authenticated request")
            .text()
            .expect("response body");
        assert_eq!(body, "auth");
        proxy.join().expect("proxy");
    }

    #[test]
    fn socks_configuration_rejects_inline_credentials_and_unsupported_auth_shapes() {
        for unsafe_url in [
            "http://proxy.test:1080",
            "socks5://user:secret@proxy.test:1080",
            "socks5://proxy.test:1080/path",
            "socks5://proxy.test:1080?token=secret",
            "socks5://proxy.test:1080#fragment",
        ] {
            assert!(SocksProxyUrl::parse(unsafe_url).is_err(), "{unsafe_url}");
        }

        let authentication = ProxyAuthentication {
            username: "proxy-user".to_owned(),
            password: CredentialRef {
                service: "DownloadManager/proxy/socks".to_owned(),
                account: "proxy-user".to_owned(),
            },
        };
        let socks4 = ProxyConfig::Socks(SocksProxyConfig {
            proxy_url: SocksProxyUrl::parse("socks4://127.0.0.1:1080").expect("SOCKS4 URL"),
            authentication: Some(authentication.clone()),
            no_proxy: Vec::new(),
        });
        assert!(matches!(
            build_proxy_client_with_credentials(&socks4, &|_| Ok("secret".to_owned())),
            Err(ProxyError::Socks4AuthenticationUnsupported)
        ));

        let socks5 = ProxyConfig::Socks(SocksProxyConfig {
            proxy_url: SocksProxyUrl::parse("socks5://127.0.0.1:1080").expect("SOCKS5 URL"),
            authentication: Some(authentication),
            no_proxy: Vec::new(),
        });
        assert!(matches!(
            build_proxy_client_with_credentials(&socks5, &|_| Ok("x".repeat(256))),
            Err(ProxyError::InvalidConfiguration)
        ));
    }

    fn read_headers(stream: &mut impl Read) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 512];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).expect("request");
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
        }
        String::from_utf8(bytes).expect("request text")
    }

    fn read_nul_terminated(stream: &mut impl Read) -> Vec<u8> {
        let mut value = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).expect("NUL-terminated value");
            if byte[0] == 0 {
                return value;
            }
            value.push(byte[0]);
        }
    }

    fn consume_socks5_connect(stream: &mut impl Read) {
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).expect("connect header");
        assert_eq!(&header[..3], &[5, 1, 0]);
        match header[3] {
            1 => {
                let mut address = [0_u8; 4];
                stream.read_exact(&mut address).expect("IPv4 address");
            }
            3 => {
                let mut length = [0_u8; 1];
                stream.read_exact(&mut length).expect("domain length");
                let mut domain = vec![0_u8; usize::from(length[0])];
                stream.read_exact(&mut domain).expect("domain");
            }
            4 => {
                let mut address = [0_u8; 16];
                stream.read_exact(&mut address).expect("IPv6 address");
            }
            value => panic!("unexpected SOCKS5 address type {value}"),
        }
        let mut port = [0_u8; 2];
        stream.read_exact(&mut port).expect("destination port");
    }
}
