//! HTTP(S) proxy client configuration and no-proxy matching.

use reqwest::{Proxy, blocking::Client};
use thiserror::Error;
use url::Url;

use crate::domain::HttpUrl;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualProxyConfig {
    pub http_proxy: Option<HttpUrl>,
    pub https_proxy: Option<HttpUrl>,
    pub no_proxy: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyConfig {
    /// Uses the platform/environment proxy discovery supported by the HTTP stack.
    System,
    /// Makes direct connections and ignores system proxy configuration.
    Direct,
    Manual(ManualProxyConfig),
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("proxy client configuration failed: {0}")]
    Client(#[from] reqwest::Error),
}

/// Builds an HTTP client for system, direct, or per-scheme manual proxy routing.
///
/// # Errors
/// Returns an error when the underlying HTTP client cannot be configured.
pub fn build_proxy_client(config: &ProxyConfig) -> Result<Client, ProxyError> {
    let builder = Client::builder();
    match config {
        ProxyConfig::System => Ok(builder.build()?),
        ProxyConfig::Direct => Ok(builder.no_proxy().build()?),
        ProxyConfig::Manual(manual) => {
            let web_proxy = manual
                .http_proxy
                .as_ref()
                .and_then(|value| Url::parse(value.as_str()).ok());
            let secure_proxy = manual
                .https_proxy
                .as_ref()
                .and_then(|value| Url::parse(value.as_str()).ok());
            let no_proxy = manual.no_proxy.clone();
            let proxy = Proxy::custom(move |target| {
                if should_bypass_proxy(target, &no_proxy) {
                    return None;
                }
                match target.scheme() {
                    "http" => web_proxy.clone(),
                    "https" => secure_proxy.clone(),
                    _ => None,
                }
            });
            Ok(builder.no_proxy().proxy(proxy).build()?)
        }
    }
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

    use super::{ManualProxyConfig, ProxyConfig, build_proxy_client, should_bypass_proxy};
    use crate::{
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
}
