//! Bounded HTTP Basic and Digest authentication challenge handling.

use md5::{Digest, Md5};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header,
};
use thiserror::Error;
use url::Url;

use crate::domain::HttpUrl;

pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("HTTP authentication request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("server requested an unsupported authentication scheme")]
    UnsupportedChallenge,
    #[error("credentials were rejected after one authentication attempt")]
    Rejected,
}

/// Performs an unauthenticated request followed by at most one Basic or Digest response.
///
/// # Errors
/// Returns an error for transport failures, unsupported/malformed challenges, or rejected
/// credentials. Authentication never loops after the credentialed attempt.
pub fn get_with_challenge(
    client: &Client,
    url: &HttpUrl,
    credentials: &Credentials,
) -> Result<Response, AuthError> {
    let initial = client.get(url.as_str()).send()?;
    if initial.status() != StatusCode::UNAUTHORIZED {
        return Ok(initial);
    }
    let challenge = initial
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::UnsupportedChallenge)?;
    let retry = if challenge
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("basic"))
    {
        client
            .get(url.as_str())
            .basic_auth(&credentials.username, Some(&credentials.password))
            .send()?
    } else if challenge
        .get(..6)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("digest"))
    {
        let authorization = digest_authorization(url, credentials, challenge)?;
        client
            .get(url.as_str())
            .header(header::AUTHORIZATION, authorization)
            .send()?
    } else {
        return Err(AuthError::UnsupportedChallenge);
    };
    if retry.status() == StatusCode::UNAUTHORIZED {
        Err(AuthError::Rejected)
    } else {
        Ok(retry)
    }
}

fn digest_authorization(
    url: &HttpUrl,
    credentials: &Credentials,
    challenge: &str,
) -> Result<String, AuthError> {
    let fields = parse_digest_fields(challenge.get(6..).ok_or(AuthError::UnsupportedChallenge)?);
    let realm = field(&fields, "realm")?;
    let nonce = field(&fields, "nonce")?;
    let algorithm = fields
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("algorithm"))
        .map_or("MD5", |(_, value)| value.as_str());
    if !algorithm.eq_ignore_ascii_case("MD5") {
        return Err(AuthError::UnsupportedChallenge);
    }
    let parsed = Url::parse(url.as_str()).map_err(|_| AuthError::UnsupportedChallenge)?;
    let mut uri = parsed.path().to_owned();
    if let Some(query) = parsed.query() {
        uri.push('?');
        uri.push_str(query);
    }
    let ha1 = md5_hex(&format!(
        "{}:{realm}:{}",
        credentials.username, credentials.password
    ));
    let ha2 = md5_hex(&format!("GET:{uri}"));
    let qop = fields
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("qop"))
        .map(|(_, value)| value.as_str());
    let cnonce = &md5_hex(&format!("{}:{nonce}", credentials.username))[..16];
    let (response, suffix) = if qop.is_some_and(|value| {
        value
            .split(',')
            .any(|candidate| candidate.trim().eq_ignore_ascii_case("auth"))
    }) {
        (
            md5_hex(&format!("{ha1}:{nonce}:00000001:{cnonce}:auth:{ha2}")),
            format!(", qop=auth, nc=00000001, cnonce=\"{cnonce}\""),
        )
    } else {
        (md5_hex(&format!("{ha1}:{nonce}:{ha2}")), String::new())
    };
    Ok(format!(
        "Digest username=\"{}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\", algorithm=MD5{suffix}",
        credentials.username
    ))
}

fn parse_digest_fields(value: &str) -> Vec<(String, String)> {
    value
        .split(',')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            Some((
                name.trim().to_owned(),
                value.trim().trim_matches('"').to_owned(),
            ))
        })
        .collect()
}

fn field<'a>(fields: &'a [(String, String)], name: &str) -> Result<&'a str, AuthError> {
    fields
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .ok_or(AuthError::UnsupportedChallenge)
}

fn md5_hex(value: &str) -> String {
    format!("{:x}", Md5::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
    };

    use reqwest::blocking::Client;

    use super::{AuthError, Credentials, get_with_challenge};
    use crate::domain::HttpUrl;

    #[test]
    fn basic_and_digest_succeed_and_bad_credentials_do_not_loop() {
        let credentials = Credentials {
            username: "user".to_owned(),
            password: "pass".to_owned(),
        };
        for (challenge, expected_fragment) in [
            ("Basic realm=\"fixture\"", "Basic dXNlcjpwYXNz"),
            (
                "Digest realm=\"fixture\", nonce=\"abc123\", qop=\"auth\", algorithm=MD5",
                "response=\"bd6f7242b784e325f6d2cb8a070ee69b\"",
            ),
        ] {
            let (url, count, server) = auth_server(challenge, Some(expected_fragment));
            let response = get_with_challenge(
                &Client::builder().build().expect("client"),
                &HttpUrl::parse(url).expect("URL"),
                &credentials,
            )
            .expect("authenticated response");
            assert!(response.status().is_success());
            server.join().expect("server");
            assert_eq!(count.load(Ordering::Acquire), 2);
        }

        let (url, count, server) = auth_server("Basic realm=\"fixture\"", None);
        assert!(matches!(
            get_with_challenge(
                &Client::builder().build().expect("client"),
                &HttpUrl::parse(url).expect("URL"),
                &credentials,
            ),
            Err(AuthError::Rejected)
        ));
        server.join().expect("server");
        assert_eq!(count.load(Ordering::Acquire), 2);
    }

    fn auth_server(
        challenge: &'static str,
        accepted_fragment: Option<&'static str>,
    ) -> (String, Arc<AtomicUsize>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let count = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&count);
        let server = thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept");
                observed.fetch_add(1, Ordering::AcqRel);
                let request = read_headers(&mut stream);
                let accepted = attempt == 1
                    && accepted_fragment.is_some_and(|fragment| request.contains(fragment));
                if accepted {
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        )
                        .expect("success");
                } else {
                    write!(
                        stream,
                        "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: {challenge}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .expect("challenge");
                }
            }
        });
        (format!("http://{address}/protected?x=1"), count, server)
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
