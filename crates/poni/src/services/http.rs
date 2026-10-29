//! Minimal HTTPS requests (GET, POST/DELETE with a JSON body, POST of raw bytes, and file downloads with progress) over WinRT `Windows.Web.Http.HttpClient`.
//!
//! Every call blocks until the response (headers and body) has arrived or the
//! timeout has passed, so use it on worker threads only, never on the UI
//! thread. Waiting costs no CPU: the calling thread sleeps on a channel that
//! the operation's `Completed` callback signals; on timeout the operation is
//! cancelled.
//!
//! [`Client`] keeps one `HttpClient` (and its connection pool) for a worker's
//! lifetime; the free function [`get`] is a one-shot convenience.

use std::fmt;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use windows::core::{h, RuntimeType, HSTRING};
use windows::Foundation::{AsyncOperationProgressHandler, AsyncOperationWithProgressCompletedHandler, AsyncStatus, IAsyncOperationWithProgress, Uri};
use windows::Storage::Streams::{DataReader, DataWriter, IBuffer, UnicodeEncoding};
use windows::Web::Http::Filters::{HttpBaseProtocolFilter, HttpCacheReadBehavior, HttpCacheWriteBehavior, HttpCookieUsageBehavior};
use windows::Web::Http::Headers::HttpMediaTypeHeaderValue;
use windows::Web::Http::{HttpBufferContent, HttpClient, HttpMethod, HttpProgress, HttpRequestMessage, HttpResponseMessage, HttpStringContent, IHttpContent};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

pub use pure::parse_retry_after;
use pure::{decode_body, network_detail};

/// Budget for one request: connect + headers + body.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);

/// Bodies larger than this are refused (nothing Poni fetches comes close).
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// A request body past this size (an IELTS answer's audio) goes as UTF-8
/// bytes: a string body is copied to UTF-16 first, then encoded again.
const BIG_BODY_BYTES: usize = 256 * 1024;

/// The methods [`Client::send`] speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

/// A complete response.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    /// The body decoded as UTF-8 (BOM stripped, invalid bytes replaced).
    pub body: String,
    /// Seconds the server asked us to wait (`Retry-After`, delta or HTTP-date).
    pub retry_after_s: Option<u64>,
}

/// Why a request produced no response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// Nothing arrived within the timeout; the request was cancelled.
    Timeout,
    /// DNS, connect or TLS failure: no working path to the server.
    Network(String),
    /// Anything else (bad URL, COM failure, oversized or unreadable body, …).
    Other(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Timeout => f.write_str("timed out"),
            FetchError::Network(s) | FetchError::Other(s) => f.write_str(s),
        }
    }
}

/// Keeps the calling thread in the multithreaded COM apartment for the
/// guard's lifetime. Balanced: `CoUninitialize` runs on drop only when the
/// matching `CoInitializeEx` succeeded (`S_OK` or `S_FALSE`). A thread that is
/// already single-threaded (`RPC_E_CHANGED_MODE`) is left alone; WinRT works
/// there too, but such a thread is probably a UI thread and must not block.
pub struct MtaGuard {
    initialized: bool,
}

impl MtaGuard {
    pub fn new() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            crate::pwarn!("http: called on a single-threaded apartment thread; requests block this thread");
        } else if let Err(e) = hr.ok() {
            crate::pwarn!("http: CoInitializeEx failed ({e})");
        }
        Self { initialized: hr.is_ok() }
    }
}

impl Default for MtaGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MtaGuard {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}

/// A reusable HTTP client that sends a fixed `User-Agent`.
///
/// Create and use it on one worker thread that is in a COM apartment (see
/// [`MtaGuard`]); drop it before that apartment is left.
///
/// Windows keeps an HTTP cache under it: a repeated GET whose answer had no
/// `Cache-Control` comes back from that cache and never reaches the server
/// (`cache_tests` below). A server whose answers change sends
/// `Cache-Control: no-store`, as api.oyenza.app does on every `/island` route
/// and oyenza.trade on the Stocks routes.
pub struct Client {
    http: HttpClient,
}

impl Client {
    pub fn new(user_agent: &str) -> Result<Self, FetchError> {
        let http = HttpClient::new().map_err(|e| other("could not create an HTTP client", &e))?;
        Self::with(http, user_agent)
    }

    /// A client of its own for requests that carry another service's
    /// sign-in (the AI tab's worker): it follows no redirect (a 3xx comes
    /// back as the answer, so no header goes on to another host), stores and
    /// sends no cookies beyond the request's own `Cookie` header, never reads
    /// or writes the disk cache, and never puts up a sign-in prompt.
    pub fn private(user_agent: &str) -> Result<Self, FetchError> {
        let filter = HttpBaseProtocolFilter::new().map_err(|e| other("could not create an HTTP filter", &e))?;
        let set = || -> windows::core::Result<()> {
            filter.SetAllowAutoRedirect(false)?;
            filter.SetAllowUI(false)?;
            filter.SetCookieUsageBehavior(HttpCookieUsageBehavior::NoCookies)?;
            let cache = filter.CacheControl()?;
            // NoCache came with Windows 10 1607; before it, MostRecent still
            // asks the server every time.
            cache.SetReadBehavior(HttpCacheReadBehavior::NoCache).or_else(|_| cache.SetReadBehavior(HttpCacheReadBehavior::MostRecent))?;
            cache.SetWriteBehavior(HttpCacheWriteBehavior::NoCache)
        };
        set().map_err(|e| other("could not set up the HTTP filter", &e))?;
        let http = HttpClient::Create(&filter).map_err(|e| other("could not create an HTTP client", &e))?;
        Self::with(http, user_agent)
    }

    /// A client that never answers from Windows' HTTP cache and writes
    /// nothing to it. The default one serves a second GET of the same URL
    /// from the cache when the server sent no `Cache-Control` (the IELTS
    /// ladder after a session: "answered in 1 ms", the server never asked).
    pub fn uncached(user_agent: &str) -> Result<Self, FetchError> {
        let http = (|| {
            let filter = HttpBaseProtocolFilter::new()?;
            let cache = filter.CacheControl()?;
            cache.SetReadBehavior(HttpCacheReadBehavior::MostRecent)?;
            cache.SetWriteBehavior(HttpCacheWriteBehavior::NoCache)?;
            HttpClient::Create(&filter)
        })()
        .map_err(|e| other("could not create an HTTP client", &e))?;
        Self::with(http, user_agent)
    }

    fn with(http: HttpClient, user_agent: &str) -> Result<Self, FetchError> {
        let headers = http.DefaultRequestHeaders().map_err(|e| other("could not set request headers", &e))?;
        let ua = HSTRING::from(user_agent);
        // The typed collection validates "product/version (comment)"; fall
        // back to the raw header if a future UA string doesn't parse.
        let parsed = headers.UserAgent().and_then(|c| c.TryParseAdd(&ua)).unwrap_or(false);
        if !parsed && !headers.TryAppendWithoutValidation(h!("User-Agent"), &ua).unwrap_or(false) {
            crate::pwarn!("http: could not set the User-Agent header");
        }
        Ok(Self { http })
    }

    /// GET `url`. Any HTTP status counts as a response; only a missing
    /// response is an error.
    pub fn get(&self, url: &str, timeout: Duration) -> Result<Response, FetchError> {
        let deadline = Instant::now() + timeout;
        let uri = Uri::CreateUri(&HSTRING::from(url)).map_err(|e| other("invalid URL", &e))?;
        let op = self.http.GetAsync(&uri).map_err(classify)?;
        let resp = wait(&op, deadline)?;
        let out = read_response(&resp, deadline);
        let _ = resp.Close();
        out
    }

    /// GET `url` with extra request headers (`Authorization`, `Origin`, …).
    pub fn get_with_headers(&self, url: &str, headers: &[(&str, &str)], timeout: Duration) -> Result<Response, FetchError> {
        self.send(Method::Get, url, headers, None, timeout)
    }

    /// `method` on `url` with extra request headers and, when given, a JSON
    /// body (sent as `application/json; charset=utf-8`).
    pub fn send(&self, method: Method, url: &str, headers: &[(&str, &str)], json: Option<&str>, timeout: Duration) -> Result<Response, FetchError> {
        let deadline = Instant::now() + timeout;
        let uri = Uri::CreateUri(&HSTRING::from(url)).map_err(|e| other("invalid URL", &e))?;
        let method = match method {
            Method::Get => HttpMethod::Get(),
            Method::Post => HttpMethod::Post(),
            Method::Delete => HttpMethod::Delete(),
        }
        .map_err(|e| other("could not create a request", &e))?;
        let request = HttpRequestMessage::Create(&method, &uri).map_err(|e| other("could not create a request", &e))?;
        let request_headers = request.Headers().map_err(|e| other("could not set request headers", &e))?;
        for (name, value) in headers {
            if !request_headers.TryAppendWithoutValidation(&HSTRING::from(*name), &HSTRING::from(*value)).unwrap_or(false) {
                return Err(FetchError::Other(format!("could not set the {name} header")));
            }
        }
        if let Some(body) = json.filter(|b| b.len() > BIG_BODY_BYTES) {
            let content = (|| {
                let writer = DataWriter::new()?;
                writer.WriteBytes(body.as_bytes())?;
                let content = HttpBufferContent::CreateFromBuffer(&writer.DetachBuffer()?)?;
                content.Headers()?.SetContentType(&HttpMediaTypeHeaderValue::Create(h!("application/json"))?)?;
                request.SetContent(&content)
            })();
            content.map_err(|e| other("could not set the request body", &e))?;
        } else if let Some(body) = json {
            let content = HttpStringContent::CreateFromStringWithEncodingAndMediaType(&HSTRING::from(body), UnicodeEncoding::Utf8, h!("application/json"))
                .map_err(|e| other("could not set the request body", &e))?;
            request.SetContent(&content).map_err(|e| other("could not set the request body", &e))?;
        }
        let op = self.http.SendRequestAsync(&request).map_err(classify)?;
        let resp = wait(&op, deadline)?;
        let out = read_response(&resp, deadline);
        let _ = resp.Close();
        let _ = request.Close();
        out
    }

    /// POST `body` as it is (a meeting's audio part), with its media type.
    pub fn post_bytes(&self, url: &str, headers: &[(&str, &str)], body: &[u8], content_type: &str, timeout: Duration) -> Result<Response, FetchError> {
        let deadline = Instant::now() + timeout;
        let uri = Uri::CreateUri(&HSTRING::from(url)).map_err(|e| other("invalid URL", &e))?;
        let request = HttpRequestMessage::Create(&HttpMethod::Post().map_err(|e| other("could not create a request", &e))?, &uri)
            .map_err(|e| other("could not create a request", &e))?;
        let request_headers = request.Headers().map_err(|e| other("could not set request headers", &e))?;
        for (name, value) in headers {
            if !request_headers.TryAppendWithoutValidation(&HSTRING::from(*name), &HSTRING::from(*value)).unwrap_or(false) {
                return Err(FetchError::Other(format!("could not set the {name} header")));
            }
        }
        let content = (|| {
            let writer = DataWriter::new()?;
            writer.WriteBytes(body)?;
            let content = HttpBufferContent::CreateFromBuffer(&writer.DetachBuffer()?)?;
            content.Headers()?.SetContentType(&HttpMediaTypeHeaderValue::Create(&HSTRING::from(content_type))?)?;
            request.SetContent(&content)
        })();
        content.map_err(|e| other("could not set the request body", &e))?;
        let op = self.http.SendRequestAsync(&request).map_err(classify)?;
        let resp = wait(&op, deadline)?;
        let out = read_response(&resp, deadline);
        let _ = resp.Close();
        let _ = request.Close();
        out
    }
}

impl Client {
    /// GET `url` as bytes (a file), up to `max_bytes`. `progress` hears the
    /// bytes received so far, on a thread-pool thread, while they arrive.
    pub fn get_bytes(
        &self,
        url: &str,
        timeout: Duration,
        max_bytes: u64,
        mut progress: impl FnMut(u64) + Send + 'static,
    ) -> Result<(u16, Vec<u8>), FetchError> {
        let deadline = Instant::now() + timeout;
        let uri = Uri::CreateUri(&HSTRING::from(url)).map_err(|e| other("invalid URL", &e))?;
        let op = self.http.GetAsync(&uri).map_err(classify)?;
        let report = AsyncOperationProgressHandler::<HttpResponseMessage, HttpProgress>::new(move |_, p| {
            progress(p.BytesReceived);
            Ok(())
        });
        let _ = op.SetProgress(&report);
        let resp = wait(&op, deadline)?;
        let out = (|| {
            let status = resp.StatusCode().map_err(|e| other("response without a status", &e))?.0.clamp(0, u16::MAX as i32) as u16;
            let content = resp.Content().map_err(|e| other("response without a body", &e))?;
            if let Ok(len) = content.Headers().and_then(|h| h.ContentLength()).and_then(|r| r.Value()) {
                if len > max_bytes {
                    let _ = content.Close();
                    return Err(FetchError::Other(format!("response too large ({len} bytes)")));
                }
            }
            let buf = wait(&content.ReadAsBufferAsync().map_err(classify)?, deadline);
            let _ = content.Close();
            let buf = buf?;
            if u64::from(buf.Length().unwrap_or(0)) > max_bytes {
                return Err(FetchError::Other("response too large".into()));
            }
            Ok((status, buffer_bytes(&buf)?))
        })();
        let _ = resp.Close();
        out
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.http.Close();
    }
}

/// GET `url`; returns (status, body). Worker threads only.
#[allow(dead_code)] // shared helper for services that need a one-off GET
pub fn get(url: &str, user_agent: &str) -> Result<(u16, String), String> {
    get_full(url, user_agent, DEFAULT_TIMEOUT).map(|r| (r.status, r.body)).map_err(|e| e.to_string())
}

/// GET `url` with a one-shot client, returning the full [`Response`]
/// (including `Retry-After`). Worker threads only.
#[allow(dead_code)] // shared helper for services that need a one-off GET
pub fn get_full(url: &str, user_agent: &str, timeout: Duration) -> Result<Response, FetchError> {
    let _com = MtaGuard::new();
    let client = Client::new(user_agent)?;
    client.get(url, timeout)
    // `client` drops (and releases its COM object) before `_com`.
}

fn read_response(resp: &HttpResponseMessage, deadline: Instant) -> Result<Response, FetchError> {
    let status = resp.StatusCode().map_err(|e| other("response without a status", &e))?;
    let status = status.0.clamp(0, u16::MAX as i32) as u16;
    let retry_after_s = header(resp, "Retry-After").and_then(|v| parse_retry_after(&v, crate::clock::unix_now()));
    let body = match resp.Content() {
        Ok(content) => {
            let body = read_body(&content, deadline);
            let _ = content.Close();
            body?
        }
        // No content object at all (e.g. some 204/304 replies).
        Err(_) => String::new(),
    };
    Ok(Response { status, body, retry_after_s })
}

fn read_body(content: &IHttpContent, deadline: Instant) -> Result<String, FetchError> {
    if let Ok(len) = content.Headers().and_then(|h| h.ContentLength()).and_then(|r| r.Value()) {
        if len > MAX_BODY_BYTES {
            return Err(FetchError::Other(format!("response too large ({len} bytes)")));
        }
    }
    let op = content.ReadAsBufferAsync().map_err(classify)?;
    let buf = wait(&op, deadline)?;
    let len = buf.Length().unwrap_or(0);
    if u64::from(len) > MAX_BODY_BYTES {
        return Err(FetchError::Other(format!("response too large ({len} bytes)")));
    }
    let bytes = buffer_bytes(&buf)?;
    Ok(decode_body(&bytes))
}

fn buffer_bytes(buf: &IBuffer) -> Result<Vec<u8>, FetchError> {
    let len = buf.Length().map_err(|e| other("unreadable response body", &e))?;
    if len == 0 {
        return Ok(Vec::new());
    }
    let reader = DataReader::FromBuffer(buf).map_err(|e| other("unreadable response body", &e))?;
    let mut bytes = vec![0u8; len as usize];
    reader.ReadBytes(&mut bytes).map_err(|e| other("unreadable response body", &e))?;
    // Give the buffer back instead of closing it with the reader.
    let _ = reader.DetachBuffer();
    Ok(bytes)
}

fn header(resp: &HttpResponseMessage, name: &str) -> Option<String> {
    let headers = resp.Headers().ok()?;
    let key = HSTRING::from(name);
    // `Lookup` fails on a missing key, so ask first.
    if !headers.HasKey(&key).unwrap_or(false) {
        return None;
    }
    headers.Lookup(&key).ok().map(|v| v.to_string_lossy())
}

/// Block until `op` finishes or `deadline` passes (then cancel it).
fn wait<T, P>(op: &IAsyncOperationWithProgress<T, P>, deadline: Instant) -> Result<T, FetchError>
where
    T: RuntimeType + 'static,
    P: RuntimeType + 'static,
{
    if op.Status().map_err(classify)? == AsyncStatus::Started {
        let (tx, rx) = mpsc::channel::<()>();
        // Runs on a thread-pool thread, or right away if `op` already finished.
        let done = AsyncOperationWithProgressCompletedHandler::new(move |_, _| {
            let _ = tx.send(());
            Ok(())
        });
        op.SetCompleted(&done).map_err(classify)?;
        let left = deadline.saturating_duration_since(Instant::now());
        if let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(left) {
            let _ = op.Cancel();
            return Err(FetchError::Timeout);
        }
    }
    match op.Status().map_err(classify)? {
        // For `Error`, GetResults returns the operation's failure code.
        AsyncStatus::Completed | AsyncStatus::Error => op.GetResults().map_err(classify),
        AsyncStatus::Canceled => Err(FetchError::Other("request cancelled".into())),
        _ => {
            // The completion handler went away without firing.
            let _ = op.Cancel();
            Err(FetchError::Timeout)
        }
    }
}

fn classify(e: windows::core::Error) -> FetchError {
    let code = e.code().0 as u32;
    // WinINet / WinHTTP errors are Win32 codes 12001..=12199 (HRESULT 0x80072EE1..).
    let win32 = if code & 0xFFFF_0000 == 0x8007_0000 { code & 0xFFFF } else { 0 };
    match win32 {
        12002 => FetchError::Timeout,
        12001..=12199 => FetchError::Network(format!("{} (0x{code:08X})", network_detail(win32))),
        _ => other("request failed", &e),
    }
}

fn other(what: &str, e: &windows::core::Error) -> FetchError {
    let msg = e.message();
    let msg = msg.trim();
    if msg.is_empty() {
        FetchError::Other(format!("{what} (0x{:08X})", e.code().0 as u32))
    } else {
        FetchError::Other(format!("{what}: {msg} (0x{:08X})", e.code().0 as u32))
    }
}

/// OS-free helpers (unit-tested on any host).
mod pure {
    use poni_core::datetime::{days_from_civil, days_in_month};

    /// Seconds to wait according to a `Retry-After` value: delta-seconds
    /// (`"120"`) or an HTTP-date (IMF-fixdate or the obsolete RFC 850 form),
    /// relative to `now_unix`. A date in the past means "now" (0).
    pub fn parse_retry_after(value: &str, now_unix: i64) -> Option<u64> {
        let v = value.trim();
        if v.is_empty() {
            return None;
        }
        if v.bytes().all(|b| b.is_ascii_digit()) {
            // An absurdly long delta saturates; callers clamp it anyway.
            return Some(v.parse::<u64>().unwrap_or(u64::MAX));
        }
        let at = parse_http_date(v)?;
        Some(at.saturating_sub(now_unix).max(0) as u64)
    }

    /// `"Sun, 06 Nov 1994 08:49:37 GMT"` or `"Sunday, 06-Nov-94 08:49:37 GMT"`
    /// → unix seconds.
    pub fn parse_http_date(s: &str) -> Option<i64> {
        let (_weekday, rest) = s.split_once(',')?;
        let mut parts = rest.split_whitespace();
        let first = parts.next()?;
        let (day, mon, year) = if first.contains('-') {
            // RFC 850: 06-Nov-94
            let mut dmy = first.split('-');
            let day = dmy.next()?;
            let mon = dmy.next()?;
            let yy: i32 = dmy.next()?.parse().ok()?;
            let year = if yy < 100 { if yy < 70 { 2000 + yy } else { 1900 + yy } } else { yy };
            (day, mon, year)
        } else {
            (first, parts.next()?, parts.next()?.parse().ok()?)
        };
        let day: u8 = day.parse().ok()?;
        let month = month_number(mon)?;
        let mut hms = parts.next()?.split(':');
        let hour: i64 = hms.next()?.parse().ok()?;
        let minute: i64 = hms.next()?.parse().ok()?;
        let second: i64 = hms.next()?.parse().ok()?;
        if hms.next().is_some() || !matches!(parts.next()?, "GMT" | "UTC") || parts.next().is_some() {
            return None;
        }
        if !(1970..=9999).contains(&year)
            || day == 0
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 60
        {
            return None;
        }
        Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
    }

    fn month_number(m: &str) -> Option<u8> {
        const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
        let m = m.to_ascii_lowercase();
        MONTHS.iter().position(|x| *x == m).map(|i| i as u8 + 1)
    }

    /// Response bytes → text: UTF-8 with an optional BOM; invalid sequences
    /// become U+FFFD instead of failing the whole response.
    pub fn decode_body(bytes: &[u8]) -> String {
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Short description of a WinINet/WinHTTP error code (Win32 12001..=12199).
    pub fn network_detail(win32: u32) -> &'static str {
        match win32 {
            12007 => "server not found",
            12029 => "could not connect",
            12030 => "connection aborted",
            12031 => "connection reset",
            12152 => "invalid server response",
            12157 | 12175 => "secure connection failed",
            12037 | 12038 | 12044 | 12045 => "certificate problem",
            _ => "network error",
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn retry_after_delta_seconds() {
            assert_eq!(parse_retry_after("120", 0), Some(120));
            assert_eq!(parse_retry_after("  7 ", 0), Some(7));
            assert_eq!(parse_retry_after("0", 0), Some(0));
            assert_eq!(parse_retry_after("99999999999999999999999", 0), Some(u64::MAX));
            assert_eq!(parse_retry_after("", 0), None);
            assert_eq!(parse_retry_after("-5", 0), None);
            assert_eq!(parse_retry_after("soon", 0), None);
        }

        #[test]
        fn retry_after_http_dates() {
            // The RFC 9110 example date.
            let t = parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT");
            assert_eq!(t, Some(784_111_777));
            assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), Some(784_111_777));
            assert_eq!(parse_retry_after("Sun, 06 Nov 1994 08:49:57 GMT", 784_111_777), Some(20));
            // Already past → wait 0.
            assert_eq!(parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", 784_200_000), Some(0));
            assert_eq!(parse_http_date("Thu, 29 Feb 2024 23:59:59 GMT"), Some(1_709_251_199));
        }

        #[test]
        fn rejects_malformed_dates() {
            assert_eq!(parse_http_date("Sun 06 Nov 1994 08:49:37 GMT"), None);
            assert_eq!(parse_http_date("Sun, 31 Feb 2023 08:49:37 GMT"), None);
            assert_eq!(parse_http_date("Sun, 06 Nov 1994 25:00:00 GMT"), None);
            assert_eq!(parse_http_date("Sun, 06 Foo 1994 08:49:37 GMT"), None);
            assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 PST"), None);
            assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49 GMT"), None);
        }

        #[test]
        fn decodes_utf8_with_bom_and_garbage() {
            assert_eq!(decode_body(b"\xEF\xBB\xBF{\"a\":1}"), "{\"a\":1}");
            assert_eq!(decode_body("caf\u{e9}".as_bytes()), "caf\u{e9}");
            assert_eq!(decode_body(b"ok\xFF"), "ok\u{FFFD}");
            assert_eq!(decode_body(b""), "");
        }

        #[test]
        fn names_common_network_errors() {
            assert_eq!(network_detail(12007), "server not found");
            assert_eq!(network_detail(12029), "could not connect");
            assert_eq!(network_detail(12100), "network error");
        }
    }
}

/// Windows' HTTP cache, against a server of the test's own on 127.0.0.1 (CI's
/// Windows unit tests): the same client and call as the Oyenza worker's.
#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// GETs one new URL twice, with a key in `Authorization`, from a server
    /// that numbers its answers and adds `headers` to each. Returns the
    /// bodies and how many GETs reached the server.
    fn ask_twice(headers: &'static str) -> (Vec<String>, usize) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("its address").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        std::thread::spawn(move || {
            for c in listener.incoming().flatten() {
                let counted = counted.clone();
                std::thread::spawn(move || answer(c, &counted, headers));
            }
        });
        // A URL no earlier run asked for: the cache outlives the process.
        let run = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let url = format!("http://127.0.0.1:{port}/island/bills?tz=%2B07%3A00&days=35&run={run}");
        let _com = MtaGuard::new();
        let client = Client::new("OyenzaStudio/test").expect("a client");
        let bodies = (0..2)
            .map(|_| client.send(Method::Get, &url, &[("Authorization", "Bearer oyzd_test")], None, DEFAULT_TIMEOUT).expect("an answer").body)
            .collect();
        (bodies, hits.load(Ordering::SeqCst))
    }

    /// One request on `c`: counted and answered only if it is a GET (a
    /// connection opened ahead and never used is neither).
    fn answer(mut c: TcpStream, counted: &AtomicUsize, headers: &str) {
        let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
        let (mut head, mut chunk) = (Vec::new(), [0u8; 4096]);
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match c.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => head.extend_from_slice(&chunk[..n]),
            }
        }
        if !head.starts_with(b"GET ") {
            return;
        }
        let n = counted.fetch_add(1, Ordering::SeqCst) + 1;
        let body = format!(r#"{{"hit":{n}}}"#);
        let _ = c.write_all(
            format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}", body.len()).as_bytes(),
        );
    }

    #[test]
    fn a_repeated_get_without_cache_headers_comes_from_windows_cache() {
        // oyenza-studio#137's IELTS self-test found this. If it ever stops
        // happening, the test below proves nothing: look at both again.
        let (bodies, hits) = ask_twice("");
        assert_eq!((hits, bodies[1].as_str()), (1, r#"{"hit":1}"#), "answers {bodies:?}");
    }

    #[test]
    fn a_repeated_get_answered_with_no_store_reaches_the_server() {
        // The headers api.oyenza.app sends on every /island route
        // (catat-server's requirePc), so Today's bills, Health steps and
        // money are asked for every time.
        let (bodies, hits) = ask_twice("Cache-Control: no-store\r\nVary: Origin\r\n");
        assert_eq!((hits, bodies[1].as_str()), (2, r#"{"hit":2}"#), "answers {bodies:?}");
    }
}
