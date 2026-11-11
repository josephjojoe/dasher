//! Refusing private addresses at connect time.
//!
//! Every server here is on localhost, so these show the refusals; that
//! public addresses still connect is covered by the guard's unit tests of
//! which ranges are private.

mod support;

use std::net::{IpAddr, Ipv4Addr};

use dasher_net::{BlockReason, Client, ErrorKind, Proxy, ResourceType, Url};
use dasher_test_server::{FixtureResponse, FixtureServer};

fn guarded() -> Client {
    Client::builder()
        .block_private_addresses(true)
        .build()
        .unwrap()
}

fn page_server() -> FixtureServer {
    FixtureServer::builder()
        .route("/page", |_| FixtureResponse::html("<p>internal</p>"))
        .start()
        .unwrap()
}

fn refused_address(error: &dasher_net::Error) -> IpAddr {
    match error.kind() {
        ErrorKind::Blocked(BlockReason::PrivateAddress(ip)) => *ip,
        other => panic!("expected a private address refusal, got {other:?}: {error}"),
    }
}

#[tokio::test]
async fn literal_private_addresses_are_refused() {
    let server = page_server();
    let target = Url::parse(&server.url("/page")).unwrap();
    assert_eq!(target.host_str(), Some("127.0.0.1"));

    let session = guarded().session();
    let error = session
        .get(target.clone(), ResourceType::Document)
        .await
        .unwrap_err();
    assert!(error.is_blocked());
    assert_eq!(refused_address(&error), IpAddr::from(Ipv4Addr::LOCALHOST));
    assert_eq!(error.url(), Some(&target));
    assert_eq!(
        error.to_string(),
        format!(
            "request blocked: 127.0.0.1 is a loopback address, and private addresses are \
             refused ({target})"
        )
    );
    assert_eq!(server.request_count(), 0);
    assert_eq!(session.activity().in_flight(), 0);

    // Other spellings of local addresses.
    let port = target.port().unwrap();
    for (url, ip) in [
        (format!("http://[::1]:{port}/"), "::1"),
        (format!("http://0.0.0.0:{port}/"), "0.0.0.0"),
        (format!("http://2130706433:{port}/"), "127.0.0.1"),
        (
            format!("http://[::ffff:127.0.0.1]:{port}/"),
            "::ffff:127.0.0.1",
        ),
        (
            "http://169.254.169.254/latest/meta-data/".into(),
            "169.254.169.254",
        ),
        ("https://10.1.2.3/".into(), "10.1.2.3"),
    ] {
        let error = session
            .get(Url::parse(&url).unwrap(), ResourceType::Fetch)
            .await
            .unwrap_err();
        assert_eq!(
            refused_address(&error),
            ip.parse::<IpAddr>().unwrap(),
            "{url}"
        );
    }
    assert_eq!(server.request_count(), 0);

    // Off by default.
    let response = Client::new()
        .unwrap()
        .session()
        .get(target, ResourceType::Document)
        .await
        .unwrap();
    assert_eq!(response.bytes().await.unwrap(), "<p>internal</p>");
}

#[tokio::test]
async fn names_are_checked_after_resolving() {
    let server = support::TlsServer::start();
    let target = Url::parse(&server.url("/page")).unwrap();
    assert_eq!(target.host_str(), Some("localhost"));

    let client = Client::builder()
        .block_private_addresses(true)
        .built_in_root_certificates(false)
        .add_root_certificate(server.certificate.clone())
        .build()
        .unwrap();
    let error = client
        .session()
        .get(target.clone(), ResourceType::Document)
        .await
        .unwrap_err();
    assert!(refused_address(&error).is_loopback(), "{error}");
    assert_eq!(server.connections(), 0);

    let client = Client::builder()
        .built_in_root_certificates(false)
        .add_root_certificate(server.certificate.clone())
        .build()
        .unwrap();
    let response = client
        .session()
        .get(target, ResourceType::Document)
        .await
        .unwrap();
    assert_eq!(response.bytes().await.unwrap(), "HTTP/2.0 /page");
}

#[tokio::test]
async fn through_a_proxy_literal_targets_and_the_proxy_are_checked() {
    let proxy = support::TestProxy::start();
    let client = Client::builder()
        .block_private_addresses(true)
        .proxy(Proxy::http(&proxy.url()).unwrap())
        .build()
        .unwrap();
    let session = client.session();

    // A literal private target, plain or tunnelled, is refused before the
    // proxy is asked.
    for url in ["http://10.0.0.1/", "https://[fd00:ec2::254]/"] {
        let error = session
            .get(Url::parse(url).unwrap(), ResourceType::Document)
            .await
            .unwrap_err();
        let expected: IpAddr = url
            .split("//")
            .nth(1)
            .unwrap()
            .trim_end_matches('/')
            .trim_matches(['[', ']'])
            .parse()
            .unwrap();
        assert_eq!(refused_address(&error), expected, "{url}");
    }

    // A name is the proxy's to resolve, but this proxy is itself on a
    // private address.
    for url in ["http://example.com/", "https://example.com/"] {
        let error = session
            .get(Url::parse(url).unwrap(), ResourceType::Document)
            .await
            .unwrap_err();
        assert_eq!(
            refused_address(&error),
            IpAddr::from(Ipv4Addr::LOCALHOST),
            "{url}"
        );
    }
    assert!(proxy.requests().is_empty());
}
