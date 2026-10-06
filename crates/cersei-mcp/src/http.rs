//! The Streamable HTTP client plugged into `rmcp`: the workspace's reqwest,
//! with every server message bounded.
//!
//! `rmcp` drives the protocol (headers `MCP-Protocol-Version`, `Mcp-Method`,
//! `Mcp-Name`, era detection, legacy sessions); this client sends the POST /
//! GET / DELETE requests and reads the answers: a JSON body is read up to
//! `max_message_bytes`, an SSE stream event by event with the same bound per
//! event. Redirects are not followed.
//!
//! The `legacy_discover_response` adaptation mirrors rmcp 3.5's own reqwest
//! client (MIT): a 4xx answer to `server/discover` is turned into a JSON-RPC
//! error for the lifecycle's era detection.

use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt};
use http::{HeaderName, HeaderValue, StatusCode};
use rmcp::model::{ClientJsonRpcMessage, ClientRequest, ErrorData, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::{Error as SseError, Sse, SseStream};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

const SESSION_HEADER: &str = "mcp-session-id";
const LAST_EVENT_ID: &str = "last-event-id";

/// Errors of the HTTP layer.
#[derive(Debug, thiserror::Error)]
pub enum HttpClientError {
    #[error("{0}")]
    Request(String),
    #[error("server message larger than {limit} bytes")]
    TooLarge { limit: usize },
}

#[derive(Clone)]
pub struct BoundedHttpClient {
    client: reqwest::Client,
    max_message_bytes: usize,
    reason: crate::stdio::CloseReason,
}

impl BoundedHttpClient {
    pub fn new(max_message_bytes: usize) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("cannot build the MCP HTTP client: {e}"))?;
        Ok(Self {
            client,
            max_message_bytes,
            reason: Default::default(),
        })
    }

    /// Set when an oversized message made this client give up.
    pub fn close_reason(&self) -> crate::stdio::CloseReason {
        self.reason.clone()
    }

    fn too_large(&self, limit: usize) -> Error {
        self.reason.set(format!(
            "the server sent a message larger than {limit} bytes"
        ));
        StreamableHttpError::Client(HttpClientError::TooLarge { limit })
    }
}

type Error = StreamableHttpError<HttpClientError>;

fn req_err(e: reqwest::Error) -> Error {
    StreamableHttpError::Client(HttpClientError::Request(e.without_url().to_string()))
}

fn apply(
    mut req: reqwest::RequestBuilder,
    auth: Option<String>,
    headers: HashMap<HeaderName, HeaderValue>,
) -> reqwest::RequestBuilder {
    if let Some(a) = auth {
        req = req.bearer_auth(a);
    }
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_bytes());
    }
    req
}

/// The body, or `None` when it is larger than `limit`.
async fn read_bounded(resp: reqwest::Response, limit: usize) -> Result<Option<Vec<u8>>, Error> {
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(req_err)?;
        if body.len() + chunk.len() > limit {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

/// Bytes of the SSE stream with an event-size limit: the count restarts at
/// each blank line (the end of an event).
fn bounded_events(
    stream: impl futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    limit: usize,
) -> BoxStream<'static, Result<Sse, SseError>> {
    struct State {
        since_event: usize,
        last: [u8; 3],
        failed: bool,
    }
    let state = Arc::new(parking_lot::Mutex::new(State {
        since_event: 0,
        last: [0; 3],
        failed: false,
    }));
    let limited = stream.map(move |chunk| {
        let chunk = chunk.map_err(|e| std::io::Error::other(e.without_url().to_string()))?;
        let mut s = state.lock();
        if s.failed {
            return Err(std::io::Error::other(
                "stream closed after an oversized event",
            ));
        }
        for &b in chunk.iter() {
            s.since_event += 1;
            let end = (b == b'\n' && s.last[2] == b'\n')
                || (b == b'\n' && s.last[2] == b'\r' && s.last[1] == b'\n');
            s.last = [s.last[1], s.last[2], b];
            if end {
                s.since_event = 0;
            }
            if s.since_event > limit {
                s.failed = true;
                return Err(std::io::Error::other(format!(
                    "SSE event larger than {limit} bytes"
                )));
            }
        }
        Ok::<Bytes, std::io::Error>(chunk)
    });
    SseStream::from_bytes_stream(limited).boxed()
}

/// A 4xx answer to `server/discover` is the legacy-server signal of the
/// lifecycle: give it back as a JSON-RPC error for that request.
fn legacy_discover_response(
    message: &ClientJsonRpcMessage,
    session_attached: bool,
    status: StatusCode,
    body: &str,
) -> Option<StreamableHttpPostResponse> {
    if session_attached
        || !status.is_client_error()
        || matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
    {
        return None;
    }
    let ClientJsonRpcMessage::Request(request) = message else {
        return None;
    };
    if !matches!(request.request, ClientRequest::DiscoverRequest(_)) {
        return None;
    }
    let error = match serde_json::from_str::<ServerJsonRpcMessage>(body) {
        Ok(ServerJsonRpcMessage::Error(error)) => error.error,
        _ => {
            ErrorData::invalid_request(format!("server/discover rejected with HTTP {status}"), None)
        }
    };
    Some(StreamableHttpPostResponse::Json(
        ServerJsonRpcMessage::error(error, Some(request.id.clone())),
        None,
    ))
}

impl StreamableHttpClient for BoundedHttpClient {
    type Error = HttpClientError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let limit = self.max_message_bytes;
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            limit,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let mut req = apply(
            self.client
                .post(uri.as_ref())
                .header("accept", "application/json, text/event-stream"),
            auth_header,
            custom_headers,
        );
        let session_attached = session_id.is_some();
        if let Some(s) = &session_id {
            req = req.header(SESSION_HEADER, s.as_ref());
        }
        let body = serde_json::to_vec(&message)?;
        let resp = req
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(req_err)?;
        let status = resp.status();
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == StatusCode::NOT_FOUND && session_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let header = |n: &str| {
            resp.headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header("content-type");
        let new_session = header(SESSION_HEADER);
        let is_reply_free = matches!(
            message,
            ClientJsonRpcMessage::Notification(_)
                | ClientJsonRpcMessage::Response(_)
                | ClientJsonRpcMessage::Error(_)
        );
        if status.is_success() && resp.content_length() == Some(0) && is_reply_free {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let json = content_type
            .as_deref()
            .is_some_and(|ct| ct.starts_with("application/json"));
        if !status.is_success() {
            let limit = self.max_message_bytes.min(1 << 20);
            let body = read_bounded(resp, limit)
                .await?
                .ok_or_else(|| self.too_large(limit))?;
            let text = String::from_utf8_lossy(&body);
            if let Some(r) = legacy_discover_response(&message, session_attached, status, &text) {
                return Ok(r);
            }
            if json {
                if let Ok(m @ ServerJsonRpcMessage::Error(_)) =
                    serde_json::from_str::<ServerJsonRpcMessage>(&text)
                {
                    return Ok(StreamableHttpPostResponse::Json(m, new_session));
                }
            }
            let preview: String = text.chars().take(200).collect();
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {preview}"),
            )));
        }
        match content_type.as_deref() {
            Some(ct) if ct.starts_with("text/event-stream") => Ok(StreamableHttpPostResponse::Sse(
                bounded_events(resp.bytes_stream(), max_sse_event_size),
                new_session,
            )),
            Some(ct) if ct.starts_with("application/json") => {
                let body = read_bounded(resp, self.max_message_bytes)
                    .await?
                    .ok_or_else(|| self.too_large(self.max_message_bytes))?;
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                    Ok(m) => Ok(StreamableHttpPostResponse::Json(m, new_session)),
                    Err(_) if is_reply_free => Ok(StreamableHttpPostResponse::Accepted),
                    Err(e) => Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                        format!("the server sent JSON that is not a JSON-RPC message: {e}"),
                    ))),
                }
            }
            _ => Err(StreamableHttpError::UnexpectedContentType(content_type)),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), Error> {
        let resp = apply(
            self.client.delete(uri.as_ref()),
            auth_header,
            custom_headers,
        )
        .header(SESSION_HEADER, session_id.as_ref())
        .send()
        .await
        .map_err(req_err)?;
        if resp.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportDeleteSession);
        }
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Error> {
        let limit = self.max_message_bytes;
        self.get_stream_with_max_sse_event_size(
            uri,
            session_id,
            last_event_id,
            auth_header,
            custom_headers,
            limit,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Error> {
        let mut req = apply(
            self.client
                .get(uri.as_ref())
                .header("accept", "text/event-stream"),
            auth_header,
            custom_headers,
        );
        if let Some(s) = session_id {
            req = req.header(SESSION_HEADER, s.as_ref());
        }
        if let Some(id) = last_event_id {
            req = req.header(LAST_EVENT_ID, id);
        }
        let resp = req.send().await.map_err(req_err)?;
        if resp.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        if !resp.status().is_success() {
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {} on the event stream", resp.status()),
            )));
        }
        Ok(bounded_events(resp.bytes_stream(), max_sse_event_size))
    }
}
