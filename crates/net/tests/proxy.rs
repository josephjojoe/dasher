//! Requests through an HTTP proxy.

mod support;

use dasher_net::{Client, ErrorKind, Proxy, ResourceType, Url, Version};
use dasher_test_server::{FixtureResponse, FixtureServer};

#[tokio::test]
async fn plain_http_goes_to_the_proxy_in_absolute_form() {
    let proxy = support::TestProxy::start();
    let server = FixtureServer::builder()
        .route("/page", |_| FixtureResponse::html("via proxy"))
        .start()
        .unwrap();
    let proxy_url = proxy.url().replace("http://", "http://user:p%40ss@");
    let client = Client::builder()
        .proxy(Proxy::http(&proxy_url).unwrap())
        .build()
        .unwrap();
    let session = client.session();
    let target = Url::parse(&server.url("/page?q=1")).unwrap();

    let response = session
        .get(target.clone(), ResourceType::Document)
        .await
        .unwrap();
    assert_eq!(response.bytes().await.unwrap(), "via proxy");
    assert_eq!(server.request_count_for("/page"), 1);
    assert_eq!(
        proxy.requests(),
        [(
            format!("GET {target} HTTP/1.1"),
            Some("Basic dXNlcjpwQHNz".to_owned())
        )]
    );
}

#[tokio::test]
async fn https_is_tunnelled_with_connect() {
    let proxy = support::TestProxy::start();
    let server = support::TlsServer::start();
    let client = Client::builder()
        .proxy(Proxy::http(&proxy.url()).unwrap())
        .built_in_root_certificates(false)
        .add_root_certificate(server.certificate.clone())
        .build()
        .unwrap();
    let session = client.session();

    for path in ["/a", "/b"] {
        let response = session
            .get(Url::parse(&server.url(path)).unwrap(), ResourceType::Fetch)
            .await
            .unwrap();
        // TLS and ALPN run end to end through the tunnel.
        assert_eq!(response.version(), Version::HTTP_2);
        assert_eq!(response.bytes().await.unwrap(), format!("HTTP/2.0 {path}"));
    }
    // One tunnel, reused for both requests.
    assert_eq!(
        proxy.requests(),
        [(
            format!("CONNECT localhost:{} HTTP/1.1", server.addr.port()),
            None
        )]
    );
}

#[test]
fn proxy_urls() {
    assert_eq!(
        Proxy::http("socks5://127.0.0.1:1080").unwrap_err().kind(),
        &ErrorKind::Config
    );
    assert_eq!(
        Proxy::http("not a url").unwrap_err().kind(),
        &ErrorKind::Config
    );
    let proxy = Proxy::http("http://user:secret@proxy.internal").unwrap();
    let debug = format!("{proxy:?}");
    assert!(debug.contains("proxy.internal:80"), "{debug}");
    assert!(!debug.contains("secret"), "{debug}");
}
