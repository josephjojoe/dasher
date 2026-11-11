//! The private address guard: refusing connections to loopback,
//! link-local, private and other non-public addresses.
//!
//! The check is made on the addresses actually connected to, after DNS, so
//! a public name that resolves (or rebinds) to `127.0.0.1` or
//! `169.254.169.254` is caught as well as a literal IP in the URL.

use std::error::Error as StdError;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};

use hyper::Uri;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::dns::{GaiResolver, Name};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tower_service::Service;

use crate::error::BoxError;

/// What kind of non-public address `ip` is, or `None` for a public one.
///
/// Refused: unspecified (`0.0.0.0/8`, `::`), loopback (`127.0.0.0/8`,
/// `::1`), link-local (`169.254.0.0/16`, `fe80::/10`), private (RFC 1918,
/// IPv6 unique local `fc00::/7` and the old site-local `fec0::/10`),
/// shared CGNAT space (`100.64.0.0/10`), multicast, broadcast and the
/// reserved `240.0.0.0/4`. IPv6 addresses that carry an IPv4 address
/// (IPv4-mapped, IPv4-compatible, NAT64 `64:ff9b::/96` and 6to4
/// `2002::/16`) are judged by that IPv4 address.
pub(crate) fn private_range(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(ip) => private_v4(ip),
        IpAddr::V6(ip) => private_v6(ip),
    }
}

fn private_v4(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, ..] = ip.octets();
    match (a, b) {
        (0, _) => Some("unspecified"),
        (127, _) => Some("loopback"),
        (169, 254) => Some("link-local"),
        (10, _) | (192, 168) => Some("private"),
        (172, b) if b & 0xf0 == 16 => Some("private"),
        (100, b) if b & 0xc0 == 64 => Some("shared (CGNAT)"),
        (224..=239, _) => Some("multicast"),
        (240..=255, _) => Some("reserved"),
        _ => None,
    }
}

fn private_v6(ip: Ipv6Addr) -> Option<&'static str> {
    let s = ip.segments();
    if ip.is_unspecified() {
        return Some("unspecified");
    }
    if ip.is_loopback() {
        return Some("loopback");
    }
    let embedded = |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
    match s {
        // IPv4-compatible (::a.b.c.d) and IPv4-mapped (::ffff:a.b.c.d).
        [0, 0, 0, 0, 0, 0 | 0xffff, hi, lo] => return private_v4(embedded(hi, lo)),
        // NAT64's well-known prefix.
        [0x64, 0xff9b, 0, 0, 0, 0, hi, lo] => return private_v4(embedded(hi, lo)),
        // 6to4.
        [0x2002, hi, lo, ..] => return private_v4(embedded(hi, lo)),
        _ => {}
    }
    match s[0] {
        x if x & 0xffc0 == 0xfe80 => Some("link-local"),
        x if x & 0xffc0 == 0xfec0 => Some("site-local"),
        x if x & 0xfe00 == 0xfc00 => Some("unique local (private)"),
        x if x & 0xff00 == 0xff00 => Some("multicast"),
        _ => None,
    }
}

/// The guard refused to connect to `0`. Travels up hyper's error chain to
/// [`Session::send`](crate::Session::send), which turns it into
/// [`BlockReason::PrivateAddress`](crate::BlockReason::PrivateAddress).
#[derive(Debug)]
pub(crate) struct AddressBlocked(pub(crate) IpAddr);

impl fmt::Display for AddressBlocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refusing to connect to {}", self.0)
    }
}

impl StdError for AddressBlocked {}

/// The address the guard refused, if `error` or anything in its source
/// chain is the guard's refusal.
pub(crate) fn blocked_address(error: &(dyn StdError + 'static)) -> Option<IpAddr> {
    let mut next = Some(error);
    while let Some(e) = next {
        if let Some(AddressBlocked(ip)) = e.downcast_ref() {
            return Some(*ip);
        }
        next = e.source();
    }
    None
}

/// Refuses `ip` if the guard is on and it isn't public.
pub(crate) fn check(guard: bool, ip: IpAddr) -> Result<(), AddressBlocked> {
    if guard && private_range(ip).is_some() {
        Err(AddressBlocked(ip))
    } else {
        Ok(())
    }
}

/// The IP address in `uri`'s host, if it is one rather than a name.
pub(crate) fn literal_host(uri: &Uri) -> Option<IpAddr> {
    let host = uri.host()?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host.parse().ok()
}

/// The system resolver, dropping the addresses the guard refuses. If none
/// is left the lookup fails with [`AddressBlocked`], naming the first.
#[derive(Clone)]
pub(crate) struct GuardedResolver {
    inner: GaiResolver,
    guard: bool,
}

type ResolveFuture =
    Pin<Box<dyn Future<Output = Result<std::vec::IntoIter<SocketAddr>, BoxError>> + Send>>;

impl Service<Name> for GuardedResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = BoxError;
    type Future = ResolveFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, name: Name) -> ResolveFuture {
        let resolving = self.inner.call(name);
        let guard = self.guard;
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = resolving.await?.collect();
            if !guard {
                return Ok(addrs.into_iter());
            }
            let first = addrs.first().copied();
            let allowed: Vec<SocketAddr> = addrs
                .into_iter()
                .filter(|addr| private_range(addr.ip()).is_none())
                .collect();
            match first {
                Some(first) if allowed.is_empty() => Err(AddressBlocked(first.ip()).into()),
                _ => Ok(allowed.into_iter()),
            }
        })
    }
}

/// Makes the TCP connections, under the guard: literal IP hosts are
/// checked here, names by [`GuardedResolver`]. Every connection dasher
/// makes, to a server or to a proxy, goes through this.
#[derive(Clone)]
pub(crate) struct GuardedHttp {
    inner: HttpConnector<GuardedResolver>,
    guard: bool,
}

impl GuardedHttp {
    pub(crate) fn new(
        guard: bool,
        configure: impl FnOnce(&mut HttpConnector<GuardedResolver>),
    ) -> Self {
        let mut inner = HttpConnector::new_with_resolver(GuardedResolver {
            inner: GaiResolver::new(),
            guard,
        });
        configure(&mut inner);
        Self { inner, guard }
    }

    pub(crate) fn guard(&self) -> bool {
        self.guard
    }
}

type ConnectFuture = Pin<Box<dyn Future<Output = Result<TokioIo<TcpStream>, BoxError>> + Send>>;

impl Service<Uri> for GuardedHttp {
    type Response = TokioIo<TcpStream>;
    type Error = BoxError;
    type Future = ConnectFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, dst: Uri) -> ConnectFuture {
        if let Some(ip) = literal_host(&dst) {
            if let Err(blocked) = check(self.guard, ip) {
                return Box::pin(async move { Err(blocked.into()) });
            }
        }
        let connecting = self.inner.call(dst);
        Box::pin(async move { Ok(connecting.await?) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(ip: &str) -> Option<&'static str> {
        private_range(ip.parse().unwrap())
    }

    #[test]
    fn private_ipv4_ranges() {
        for (ip, expected) in [
            ("0.0.0.0", "unspecified"),
            ("0.1.2.3", "unspecified"),
            ("127.0.0.1", "loopback"),
            ("127.255.255.254", "loopback"),
            ("169.254.169.254", "link-local"),
            ("10.0.0.1", "private"),
            ("10.255.255.255", "private"),
            ("172.16.0.1", "private"),
            ("172.31.255.255", "private"),
            ("192.168.1.1", "private"),
            ("100.64.0.1", "shared (CGNAT)"),
            ("100.100.100.200", "shared (CGNAT)"),
            ("100.127.255.255", "shared (CGNAT)"),
            ("224.0.0.1", "multicast"),
            ("240.0.0.1", "reserved"),
            ("255.255.255.255", "reserved"),
        ] {
            assert_eq!(range(ip), Some(expected), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "172.15.255.255",
            "172.32.0.0",
            "100.63.255.255",
            "100.128.0.0",
            "169.253.0.1",
            "192.169.0.1",
            "11.0.0.1",
            "223.255.255.255",
        ] {
            assert_eq!(range(ip), None, "{ip}");
        }
    }

    #[test]
    fn private_ipv6_ranges() {
        for (ip, expected) in [
            ("::", "unspecified"),
            ("::1", "loopback"),
            ("fe80::1", "link-local"),
            ("febf::1", "link-local"),
            ("fec0::1", "site-local"),
            ("fc00::1", "unique local (private)"),
            ("fd00:ec2::254", "unique local (private)"),
            ("ff02::1", "multicast"),
            ("::ffff:127.0.0.1", "loopback"),
            ("::ffff:169.254.169.254", "link-local"),
            ("::10.0.0.1", "private"),
            ("64:ff9b::a9fe:a9fe", "link-local"),
            ("2002:7f00:1::", "loopback"),
            ("2002:c0a8:101::1", "private"),
        ] {
            assert_eq!(range(ip), Some(expected), "{ip}");
        }
        for ip in [
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:808:808::1",
            "fe00::1",
        ] {
            assert_eq!(range(ip), None, "{ip}");
        }
    }

    #[test]
    fn literal_hosts() {
        let host = |uri: &str| literal_host(&uri.parse().unwrap());
        assert_eq!(host("http://127.0.0.1:8080/"), Some([127, 0, 0, 1].into()));
        assert_eq!(host("https://[::1]/"), Some(Ipv6Addr::LOCALHOST.into()));
        assert_eq!(host("https://example.com/"), None);
        assert_eq!(host("/relative"), None);
    }

    #[test]
    fn refusals_are_found_in_error_chains() {
        let ip: IpAddr = [10, 0, 0, 1].into();
        let boxed: BoxError = AddressBlocked(ip).into();
        assert_eq!(blocked_address(&*boxed), Some(ip));

        #[derive(Debug)]
        struct Wrapper(BoxError);
        impl fmt::Display for Wrapper {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("wrapper")
            }
        }
        impl StdError for Wrapper {
            fn source(&self) -> Option<&(dyn StdError + 'static)> {
                Some(&*self.0)
            }
        }
        assert_eq!(blocked_address(&Wrapper(boxed)), Some(ip));
        assert_eq!(blocked_address(&Wrapper("other".into())), None);
    }
}
