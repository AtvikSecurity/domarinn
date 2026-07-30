//! The stub server's wire contract, pinned by speaking raw TCP to it.
//!
//! No client crate on purpose: what is under test *is* the framing — when the
//! stub replies, what it records, and whether a miss is something a real
//! client can parse.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::common::{stub_script, stub_server};

/// Send one raw request to a stub and return the whole response, headers and
/// all. Raw rather than a client crate because what is under test *is* the
/// framing.
fn raw_request(url: &str, request: &str) -> String {
    let addr = url.strip_prefix("http://").expect("stub url is http");
    let mut sock = TcpStream::connect(addr).expect("stub is listening");
    sock.write_all(request.as_bytes()).unwrap();
    sock.flush().unwrap();
    let mut response = String::new();
    // The stub answers `connection: close`, so read-to-EOF terminates.
    sock.read_to_string(&mut response).unwrap();
    response
}

fn post(path: &str, body: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nhost: stub\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The declared `content-length` of a response, for comparison against what it
/// actually sent.
fn declared_length(response: &str) -> usize {
    response
        .to_lowercase()
        .split("content-length:")
        .nth(1)
        .and_then(|rest| rest.split("\r\n").next())
        .and_then(|v| v.trim().parse().ok())
        .expect("response declares a content-length")
}

fn body_of(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .expect("response has a header/body split")
        .1
}

/// The stub must wait for a declared request body before replying.
///
/// It used to answer at end-of-headers, which is fine for the GETs its first
/// callers made but resets the connection under a POST: the client is still
/// writing when the socket closes, so it reports a transport error instead of
/// the status under test. Every model provider POSTs, so without this the
/// example harness could not stub a single one of them.
///
/// The headers and body are sent as two writes *on purpose*. Sent as one, the
/// kernel hands the server both in its first `read`, and a stub that never
/// waits still looks correct — so the obvious version of this test passes
/// against the very bug it is meant to catch.
#[test]
fn the_stub_waits_for_a_request_body_before_replying() {
    let (url, server) = stub_script(
        vec![("/v1/messages", vec![r#"{"ok":true}"#.to_string()])],
        1,
        Duration::from_secs(10),
    );

    let body = r#"{"model":"m","messages":[]}"#;
    let addr = url.strip_prefix("http://").expect("stub url is http");
    let mut sock = TcpStream::connect(addr).expect("stub is listening");
    sock.write_all(
        format!(
            "POST /v1/messages HTTP/1.1\r\nhost: stub\r\ncontent-type: application/json\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    )
    .unwrap();
    sock.flush().unwrap();

    // Nothing may come back yet: the request is incomplete, and a reply here is
    // exactly what leaves a real client writing into a closed socket.
    sock.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut early = [0u8; 128];
    match sock.read(&mut early) {
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
        Ok(n) => panic!(
            "the stub replied before the body arrived: {:?}",
            String::from_utf8_lossy(&early[..n])
        ),
        Err(e) => panic!("unexpected error reading from the stub: {e}"),
    }

    sock.write_all(body.as_bytes()).unwrap();
    sock.flush().unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut response = String::new();
    sock.read_to_string(&mut response).unwrap();

    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "a POST with a body must be answered once complete, got: {response:?}"
    );
    assert_eq!(body_of(&response), r#"{"ok":true}"#);

    // Guard against a vacuous pass: prove the stub routed *this* request rather
    // than the assertions above passing on some unrelated reply — and that it
    // recorded the whole request, body included, which is what lets the
    // examples harness assert on what actually reached the wire.
    let served = server.join().unwrap();
    assert_eq!(
        served.len(),
        1,
        "the stub did not record the POST it answered"
    );
    assert!(
        served[0].starts_with("POST /v1/messages HTTP/1.1"),
        "recorded request lost its request line: {:?}",
        served[0]
    );
    assert!(
        served[0].ends_with(body),
        "recorded request lost its body: {:?}",
        served[0]
    );
}

/// `stub_server` shares the same drain, and is what `share` / `ci-summary`
/// exercise — those POST a whole run document, the largest body any test sends.
#[test]
fn the_one_shot_stub_also_drains_a_body_and_returns_it() {
    let (url, server) = stub_server("200 OK", r#"{"url":"https://domarinn.test/runs/x"}"#);

    let sent = r#"{"schema_version":2,"cases":[]}"#;
    let response = raw_request(&url, &post("/api/v1/runs", sent));

    assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response:?}");
    let captured = String::from_utf8(server.join().unwrap()).unwrap();
    assert!(
        captured.ends_with(sent),
        "the stub must hand back the full body it drained, got: {captured:?}"
    );
}

/// A route answers a *sequence*, not one fixed body.
///
/// This is what makes a `similar` example honest: the assertion embeds the
/// output and the reference and takes their cosine, so a stub that returned one
/// vector for both would score 1.0 against itself and the threshold on the page
/// would be untested at every value.
#[test]
fn a_route_serves_a_different_body_per_request_then_repeats_the_last() {
    let (url, server) = stub_script(
        vec![(
            "/embeddings",
            vec![r#"{"n":1}"#.to_string(), r#"{"n":2}"#.to_string()],
        )],
        3,
        Duration::from_secs(10),
    );

    let bodies: Vec<String> = (0..3)
        .map(|_| body_of(&raw_request(&url, &post("/embeddings", "{}"))).to_string())
        .collect();

    assert_eq!(
        bodies,
        vec![r#"{"n":1}"#, r#"{"n":2}"#, r#"{"n":2}"#],
        "the script must advance per request, then hold on its last body"
    );
    assert_eq!(server.join().unwrap().len(), 3);
}

/// An unrouted request must produce a 404 a client can actually parse.
///
/// The 404 used to hardcode `content-length: 11` for the 12-byte body
/// `{"error":""}`, so a client reading by length got truncated JSON and
/// reported a deserialization failure — hiding the fact that it had simply
/// asked for a path no route matched.
#[test]
fn an_unrouted_request_gets_a_parseable_404_that_names_the_path() {
    let (url, server) = stub_script(
        vec![("/v1/messages", vec![r#"{"ok":true}"#.to_string()])],
        1,
        Duration::from_secs(10),
    );

    let response = raw_request(&url, &post("/v1/nope", "{}"));

    assert!(
        response.starts_with("HTTP/1.1 404 Not Found"),
        "got: {response:?}"
    );
    let body = body_of(&response);
    assert_eq!(
        declared_length(&response),
        body.len(),
        "content-length must match the body it describes, or the client truncates it"
    );
    serde_json::from_str::<serde_json::Value>(body)
        .unwrap_or_else(|e| panic!("the 404 body must be valid JSON, got {body:?}: {e}"));
    assert!(
        body.contains("/v1/nope"),
        "the 404 must name the unrouted path so the operator can see what missed: {body:?}"
    );

    server.join().unwrap();
}
