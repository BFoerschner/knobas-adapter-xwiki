//! The component's one way out: `wasi:http` 0.2's outgoing handler.
//!
//! The host decides what leaves. A request goes only to a host the manifest
//! declares (here `{base_url}`, the instance the reader configured), and
//! the host attaches the credential to it: `Authorization: Basic` from the
//! config's `username` and the secret the reader typed, which this
//! component never sees. Every response from the instance reaches us with
//! the secret redacted. So this module sets no `Authorization` header.
//!
//! The host follows no redirect and keeps no cookie. XWiki answers a REST
//! call with JSON when asked (`Accept: application/json`); nothing here
//! needs a session.
//!
//! On a non-WASM target (what `cargo test` builds) there is no outgoing
//! handler: every request fails as unreachable, and only the pure parts of
//! the adapter are tested there.

use knobas_adapter_sdk::SourceError;

/// One response, read whole.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    /// Header names lower-cased, in the order they came.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// The first header named `name` (lower case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The body as text, lossily.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body as JSON of type `T`, or a protocol error naming `what`.
    pub fn json<T: serde::de::DeserializeOwned>(&self, what: &str) -> Result<T, SourceError> {
        serde_json::from_slice(&self.body).map_err(|e| {
            SourceError::protocol(format!("{what}: the answer is not the JSON expected: {e}"))
        })
    }
}

/// A request's method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Put,
    Post,
}

/// A request body and its media type.
pub struct Body<'a> {
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

/// Map a response status onto knobas' fault classes, the way every knobas
/// adapter does: 401 and 403 are the credential's, 2xx is success, anything
/// else is a protocol error carrying its status.
pub fn check(response: Response, what: &str) -> Result<Response, SourceError> {
    match response.status {
        200..=299 => Ok(response),
        401 | 403 => Err(SourceError::Unauthorized {
            status: Some(response.status),
        }),
        status => {
            let mut text = response.text();
            text.truncate(300);
            Err(SourceError::Protocol {
                status: Some(status),
                message: format!("{what}: HTTP {status}: {}", text.trim()),
            })
        }
    }
}

/// Send one request to `url` and read the whole answer.
///
/// A transport failure (refused, DNS, TLS, timeout) is
/// [`SourceError::Unreachable`]: the host reports it as a bare `wasi:http`
/// error code, so the message is that code. A request the host's allowlist
/// refuses reads the same way (`connection-refused`).
#[cfg(target_family = "wasm")]
pub fn send(method: Method, url: &str, body: Option<Body<'_>>) -> Result<Response, SourceError> {
    use wasip2::http::outgoing_handler;
    use wasip2::http::types::{
        Fields, Method as WasiMethod, OutgoingBody, OutgoingRequest, Scheme,
    };
    use wasip2::io::streams::StreamError;

    let unreachable = |what: &str| SourceError::Unreachable(format!("{url}: {what}"));
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| SourceError::protocol(format!("{url:?} is not an absolute URL")))?;
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, "/"),
    };

    let headers = Fields::new();
    let header = |name: &str, value: &[u8]| {
        headers
            .append(name, value)
            .map_err(|e| SourceError::protocol(format!("header {name} refused: {e:?}")))
    };
    header("accept", b"application/json")?;
    if let Some(body) = &body {
        header("content-type", body.content_type.as_bytes())?;
        header("content-length", body.bytes.len().to_string().as_bytes())?;
    }
    let request = OutgoingRequest::new(headers);
    let scheme = match scheme {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        other => Scheme::Other(other.to_owned()),
    };
    request
        .set_scheme(Some(&scheme))
        .map_err(|()| SourceError::protocol("scheme refused"))?;
    request
        .set_authority(Some(authority))
        .map_err(|()| SourceError::protocol("authority refused"))?;
    request
        .set_path_with_query(Some(path))
        .map_err(|()| SourceError::protocol("path refused"))?;
    request
        .set_method(&match method {
            Method::Get => WasiMethod::Get,
            Method::Put => WasiMethod::Put,
            Method::Post => WasiMethod::Post,
        })
        .map_err(|()| SourceError::protocol("method refused"))?;

    let outgoing = request
        .body()
        .map_err(|()| SourceError::protocol("no request body"))?;
    let pending =
        outgoing_handler::handle(request, None).map_err(|e| unreachable(&format!("{e:?}")))?;
    if let Some(body) = &body {
        let stream = outgoing
            .write()
            .map_err(|()| SourceError::protocol("no request body stream"))?;
        for chunk in body.bytes.chunks(4096) {
            stream
                .blocking_write_and_flush(chunk)
                .map_err(|e| unreachable(&format!("writing the body: {e:?}")))?;
        }
        drop(stream);
    }
    // A body the host stopped reading cannot finish; the response says why.
    let _ = OutgoingBody::finish(outgoing, None);

    pending.subscribe().block();
    // A write whose connection never opened did nothing, and may go again.
    // One that went out and got no answer may have landed: knobas holds it
    // for a person to check rather than sending it twice.
    let lost = |error: String| {
        let never_opened = [
            "ConnectionRefused",
            "Dns",
            "DestinationNotFound",
            "DestinationUnavailable",
            "ConnectionTimeout",
            "Tls",
        ]
        .iter()
        .any(|code| error.contains(code));
        if method == Method::Get || never_opened {
            unreachable(&error)
        } else {
            SourceError::Unconfirmed {
                status: None,
                message: format!("{url}: no answer to the write: {error}"),
            }
        }
    };
    let response = pending
        .get()
        .ok_or_else(|| lost("no response after the wait".into()))?
        .map_err(|()| unreachable("the response was taken twice"))?
        .map_err(|e| lost(format!("{e:?}")))?;
    let status = response.status();
    let headers = response
        .headers()
        .entries()
        .into_iter()
        .map(|(name, value)| {
            (
                name.to_ascii_lowercase(),
                String::from_utf8_lossy(&value).into_owned(),
            )
        })
        .collect();
    let incoming = response
        .consume()
        .map_err(|()| unreachable("no response body"))?;
    let stream = incoming
        .stream()
        .map_err(|()| unreachable("no response body stream"))?;
    let mut bytes = Vec::new();
    loop {
        match stream.blocking_read(64 * 1024) {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(StreamError::Closed) => break,
            Err(StreamError::LastOperationFailed(error)) => {
                return Err(unreachable(&format!(
                    "reading the body: {}",
                    error.to_debug_string()
                )));
            }
        }
    }
    drop(stream);
    Ok(Response {
        status,
        headers,
        body: bytes,
    })
}

/// No outgoing handler off WASM: see the module docs.
#[cfg(not(target_family = "wasm"))]
pub fn send(method: Method, url: &str, body: Option<Body<'_>>) -> Result<Response, SourceError> {
    let _ = (method, body);
    Err(SourceError::Unreachable(format!(
        "{url}: no network off wasm32-wasip2"
    )))
}

/// Percent-encode one URL path segment or query value (RFC 3986 unreserved
/// characters pass, every other byte is `%XX`).
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_segment_is_encoded_byte_by_byte() {
        assert_eq!(encode("Tidewater"), "Tidewater");
        assert_eq!(encode("On-call handbook"), "On-call%20handbook");
        assert_eq!(encode("a/b.c"), "a%2Fb.c");
        assert_eq!(encode("Zürich"), "Z%C3%BCrich");
    }

    #[test]
    fn a_status_maps_onto_the_fault_classes() {
        let r = |status| Response {
            status,
            headers: Vec::new(),
            body: b"nope".to_vec(),
        };
        assert!(check(r(200), "x").is_ok());
        assert!(matches!(
            check(r(401), "x"),
            Err(SourceError::Unauthorized { status: Some(401) })
        ));
        assert!(matches!(
            check(r(403), "x"),
            Err(SourceError::Unauthorized { status: Some(403) })
        ));
        assert!(matches!(
            check(r(500), "x"),
            Err(SourceError::Protocol {
                status: Some(500),
                ..
            })
        ));
    }
}
