//! Loopback round-trip of the no-JS search server (net feature): start `serve`,
//! make a real HTTP GET to `/api/search`, and assert the JSON response.
#![cfg(feature = "net")]

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use websearch::index::{DocFields, Index};
use websearch::serve::{serve, serve_with_limits, ServeLimits};
use websearch::SearchServer;

#[tokio::test]
async fn api_search_over_a_socket() {
    let mut ix = Index::new();
    ix.upsert_document(
        "http://a/rust",
        DocFields {
            title: "Rust guide",
            body: "learning rust programming today",
            host: "a",
            lang: "en",
            fetched_at: 1_700_000_000.0,
            http_status: 200,
            ..DocFields::default()
        },
    );
    let server = Arc::new(SearchServer::new(
        Arc::new(Mutex::new(ix)),
        "http://127.0.0.1",
    ));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(serve(listener, server));

    let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    sock.write_all(b"GET /api/search?q=rust HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = Vec::new();
    sock.read_to_end(&mut resp).await.unwrap();
    let text = String::from_utf8_lossy(&resp);

    assert!(text.starts_with("HTTP/1.1 200 OK"), "resp: {text}");
    assert!(text.contains("application/json"));
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    assert!(body.contains("\"url\":\"http://a/rust\""), "body: {body}");
    assert!(body.contains("\"total\":1"));

    handle.abort();
}

fn empty_server() -> Arc<SearchServer> {
    Arc::new(SearchServer::new(
        Arc::new(Mutex::new(Index::new())),
        "http://127.0.0.1",
    ))
}

/// The accept loop must have a read deadline. Without one, a request head
/// with no blank line (`GET / HTTP/1.1\r\n` then silence) waits forever: 500
/// such connections were all still open after 2 s, at 1311 file descriptors.
///
/// With the deadline the server closes the connection itself and the client's
/// read hits EOF. Without it, the read never completes and the outer timeout
/// fires.
#[tokio::test]
async fn a_half_open_request_is_closed_by_the_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let limits = ServeLimits {
        max_connections: 8,
        request_timeout: Duration::from_millis(200),
    };
    let handle = tokio::spawn(serve_with_limits(listener, empty_server(), limits));

    let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    // No trailing "\r\n\r\n": the head is never complete.
    sock.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), sock.read_to_end(&mut buf)).await;
    assert!(
        read.is_ok(),
        "the server never closed a half-open request: the connection is still parked"
    );

    handle.abort();
}

/// Connections must be capped: the permit is taken before `accept()`, so with
/// a cap of 1 a second connection isn't even accepted until the first is
/// done.
#[tokio::test]
async fn the_connection_cap_defers_the_next_accept() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let limits = ServeLimits {
        max_connections: 1,
        request_timeout: Duration::from_secs(30),
    };
    let handle = tokio::spawn(serve_with_limits(listener, empty_server(), limits));

    // Occupy the single permit with a half-open request.
    let mut hog = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    hog.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

    // `connect` returning doesn't mean the server accepted it: the kernel
    // completes the handshake into the listen backlog on its own, so `hog` may
    // still be queued and the only permit unspent. Rather than sleep and hope,
    // wait for the observable effect: send a complete request on a throwaway
    // connection and see whether it gets answered. A reply means the accept loop
    // is still serving; a read timeout means it isn't, which is the state the
    // assertion below needs.
    //
    // The timeout shows the loop isn't serving anyone, not *why*: under load the
    // server task might just not be scheduled yet. The assertion holds either
    // way, because the backlog is FIFO: `hog` was queued first and, once
    // accepted, holds the only permit for its full 30 s `request_timeout`, so
    // `second` can't be served before it.
    //
    // Each probe is dropped at the end of its iteration: one left open would sit
    // in the backlog ahead of `second`, and one holding the permit with an
    // incomplete head would wait out the whole `request_timeout`.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut probe = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        probe
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut byte = [0u8; 1];
        if tokio::time::timeout(Duration::from_millis(100), probe.read(&mut byte))
            .await
            .is_err()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server kept answering new connections with max_connections = 1"
        );
        // Answered, so the cap is not spent yet. Space the retries out rather
        // than spinning: this path only runs when the cap is broken, and a tight
        // connect loop would leave thousands of sockets in TIME_WAIT on the way
        // to the failure.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A second, complete request cannot be served while the cap is spent.
    let mut second = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    second
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = [0u8; 512];
    let early = tokio::time::timeout(Duration::from_millis(400), second.read(&mut buf)).await;
    assert!(
        early.is_err(),
        "a connection past the cap was served anyway: the accept loop is unbounded"
    );

    // Freeing the first connection lets the loop accept the queued one.
    drop(hog);
    let n = tokio::time::timeout(Duration::from_secs(5), second.read(&mut buf))
        .await
        .expect("queued connection was never accepted")
        .expect("read failed");
    assert!(
        String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200 OK"),
        "queued connection got a bad reply"
    );

    handle.abort();
}
