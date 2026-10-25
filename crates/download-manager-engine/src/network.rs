//! Address-family connection fallback using system-resolved socket addresses.

use std::{
    io,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    time::Duration,
};

use thiserror::Error;

#[derive(Debug)]
pub struct ConnectedSocket {
    pub stream: TcpStream,
    pub address: SocketAddr,
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("system DNS resolution failed: {0}")]
    Resolve(#[source] io::Error),
    #[error("host resolved to no addresses")]
    NoAddresses,
    #[error("all IPv6/IPv4 connection attempts failed: {0}")]
    Exhausted(#[source] io::Error),
}

/// Resolves with the operating system and tries each IPv6/IPv4 address with a bounded timeout.
///
/// # Errors
/// Returns an error when resolution yields no addresses or every connection attempt fails.
pub fn connect_host(
    host: &str,
    port: u16,
    per_address_timeout: Duration,
) -> Result<ConnectedSocket, ConnectError> {
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(ConnectError::Resolve)?
        .collect::<Vec<_>>();
    connect_addresses(&addresses, per_address_timeout)
}

/// Tries already resolved addresses in resolver order, allowing one family to fail before the
/// other succeeds.
///
/// # Errors
/// Returns an error when the list is empty or every bounded attempt fails.
pub fn connect_addresses(
    addresses: &[SocketAddr],
    per_address_timeout: Duration,
) -> Result<ConnectedSocket, ConnectError> {
    if addresses.is_empty() {
        return Err(ConnectError::NoAddresses);
    }
    let mut last_error = None;
    for &address in addresses {
        match TcpStream::connect_timeout(&address, per_address_timeout) {
            Ok(stream) => return Ok(ConnectedSocket { stream, address }),
            Err(error) => last_error = Some(error),
        }
    }
    Err(ConnectError::Exhausted(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AddrNotAvailable, "no address attempted")
    })))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{Ipv6Addr, SocketAddr, SocketAddrV6, TcpListener},
        thread,
        time::Duration,
    };

    use super::{connect_addresses, connect_host};

    #[test]
    fn dual_stack_fixture_falls_back_after_failed_ipv6_attempt() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("IPv4 fixture");
        let ipv4 = listener.local_addr().expect("fixture address");
        let ipv6 = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, ipv4.port(), 0, 0));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fallback");
            stream.write_all(b"ok").expect("response");
        });
        let mut connected =
            connect_addresses(&[ipv6, ipv4], Duration::from_millis(100)).expect("IPv4 fallback");
        assert!(connected.address.is_ipv4());
        let mut body = [0_u8; 2];
        connected.stream.read_exact(&mut body).expect("body");
        assert_eq!(&body, b"ok");
        server.join().expect("server");

        let resolved = connect_host("localhost", ipv4.port(), Duration::from_millis(10));
        assert!(resolved.is_err(), "closed fixture port must exhaust safely");
    }
}
