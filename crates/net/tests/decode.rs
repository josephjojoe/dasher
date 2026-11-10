//! Content-Encoding decoding against the fixture server.

use async_compression::tokio::bufread::{
    BrotliEncoder, DeflateEncoder, GzipEncoder, ZlibEncoder, ZstdEncoder,
};
use async_compression::{Level, zstd::CParameter};
use dasher_net::{Client, ErrorKind, Method, Request, ResourceType, Url};
use dasher_test_server::{FixtureResponse, FixtureServer};
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Clone, Copy)]
enum Codec {
    Gzip,
    Zlib,
    RawDeflate,
    Brotli,
    Zstd,
    /// zstd with a 32 MiB window, beyond what HTTP decoders must accept.
    ZstdHugeWindow,
}

async fn encode(codec: Codec, data: &[u8]) -> Vec<u8> {
    async fn read_all(mut reader: impl AsyncRead + Unpin) -> Vec<u8> {
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        out
    }
    match codec {
        Codec::Gzip => read_all(GzipEncoder::new(data)).await,
        Codec::Zlib => read_all(ZlibEncoder::new(data)).await,
        Codec::RawDeflate => read_all(DeflateEncoder::new(data)).await,
        Codec::Brotli => read_all(BrotliEncoder::new(data)).await,
        Codec::Zstd => read_all(ZstdEncoder::new(data)).await,
        Codec::ZstdHugeWindow => {
            read_all(ZstdEncoder::with_quality_and_params(
                data,
                Level::Default,
                &[CParameter::window_log(25)],
            ))
            .await
        }
    }
}

/// Some text that compresses well but not trivially.
fn sample(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(
            format!("<li data-i=\"{i}\">item {}</li>\n", i * 7919 % 1000).as_bytes(),
        );
        i += 1;
    }
    out.truncate(len);
    out
}

fn serve(path: &str, content_encoding: &str, body: Vec<u8>) -> FixtureServer {
    let content_encoding = content_encoding.to_owned();
    FixtureServer::builder()
        .route(path, move |_| {
            FixtureResponse::html(body.clone()).with_header("content-encoding", &content_encoding)
        })
        .start()
        .unwrap()
}

fn url(server: &FixtureServer, path: &str) -> Url {
    Url::parse(&server.url(path)).unwrap()
}

#[tokio::test]
async fn decodes_every_supported_encoding() {
    let original = sample(200_000);
    for (codec, name) in [
        (Codec::Gzip, "gzip"),
        (Codec::Gzip, "x-gzip"),
        (Codec::Zlib, "deflate"),
        (Codec::RawDeflate, "deflate"),
        (Codec::Brotli, "br"),
        (Codec::Zstd, "zstd"),
    ] {
        let encoded = encode(codec, &original).await;
        assert!(encoded.len() < original.len() / 4, "{name}");
        let server = serve("/page", name, encoded);
        let session = Client::new().unwrap().session();
        let mut response = session
            .get(url(&server, "/page"), ResourceType::Document)
            .await
            .unwrap();

        let mut decoded = Vec::new();
        while let Some(chunk) = response.chunk().await.unwrap() {
            // Decoded output arrives in bounded pieces.
            assert!(chunk.len() <= 16 * 1024, "{name}: {}", chunk.len());
            decoded.extend_from_slice(&chunk);
        }
        assert!(decoded == original, "{name}");
        assert_eq!(session.activity().in_flight(), 0);
        assert_eq!(
            server.requests()[0].header("accept-encoding"),
            Some("gzip, deflate, br, zstd")
        );
    }
}

#[tokio::test]
async fn decodes_stacked_encodings() {
    let original = sample(50_000);
    // Applied gzip first, then brotli.
    let encoded = encode(Codec::Brotli, &encode(Codec::Gzip, &original).await).await;
    let server = serve("/page", "gzip, br", encoded);
    let session = Client::new().unwrap().session();
    let response = session
        .get(url(&server, "/page"), ResourceType::Document)
        .await
        .unwrap();
    assert!(response.bytes().await.unwrap() == original);
}

#[tokio::test]
async fn limits_the_decoded_size() {
    // About 10 KB of gzip that inflates to 10 MB.
    let bomb = encode(Codec::Gzip, &vec![0u8; 10 * 1024 * 1024]).await;
    assert!(bomb.len() < 64 * 1024);
    let server = serve("/bomb", "gzip", bomb);
    let client = Client::builder()
        .max_response_size(1024 * 1024)
        .build()
        .unwrap();
    let session = client.session();
    let response = session
        .get(url(&server, "/bomb"), ResourceType::Fetch)
        .await
        .unwrap();
    let error = response.bytes().await.unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::TooLarge { limit: 1024 * 1024 });
    assert_eq!(session.activity().in_flight(), 0);
}

#[tokio::test]
async fn refuses_oversized_zstd_windows() {
    let encoded = encode(Codec::ZstdHugeWindow, &sample(10_000)).await;
    let server = serve("/page", "zstd", encoded);
    let session = Client::new().unwrap().session();
    let response = session
        .get(url(&server, "/page"), ResourceType::Fetch)
        .await
        .unwrap();
    assert_eq!(
        response.bytes().await.unwrap_err().kind(),
        &ErrorKind::Decode
    );
}

#[tokio::test]
async fn reports_corrupt_and_unsupported_encodings() {
    let server = serve("/corrupt", "gzip", b"definitely not gzip".to_vec());
    let session = Client::new().unwrap().session();
    let response = session
        .get(url(&server, "/corrupt"), ResourceType::Fetch)
        .await
        .unwrap();
    let error = response.bytes().await.unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Decode);
    assert_eq!(error.url().unwrap().path(), "/corrupt");

    let server = serve("/compress", "compress", b"whatever".to_vec());
    let error = session
        .get(url(&server, "/compress"), ResourceType::Fetch)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Decode);
    assert_eq!(session.activity().in_flight(), 0);
}

#[tokio::test]
async fn empty_encoded_bodies_are_fine() {
    let server = FixtureServer::builder()
        .route("/gone", |_| {
            FixtureResponse::new(204).with_header("content-encoding", "gzip")
        })
        .route("/head", |_| {
            FixtureResponse::text("not really gzip").with_header("content-encoding", "br")
        })
        .start()
        .unwrap();
    let session = Client::new().unwrap().session();

    let response = session
        .get(url(&server, "/gone"), ResourceType::Fetch)
        .await
        .unwrap();
    // Nothing to read, so the request is already done.
    assert_eq!(session.activity().in_flight(), 0);
    assert_eq!(response.bytes().await.unwrap(), "");

    let head = Request::new(Method::HEAD, url(&server, "/head"), ResourceType::Fetch);
    let response = session.send(head).await.unwrap();
    assert_eq!(response.bytes().await.unwrap(), "");
}
