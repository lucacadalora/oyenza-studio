//! HTTPS requests on the Mac (Apple's URLSession), with the same small API as
//! the Windows app's `services/http.rs`, so the shared lyrics, Apple Music
//! and add-on code (`services/lyrics.rs`, `services/applemusic.rs`,
//! `services/addons.rs`) runs unchanged.
//!
//! Every call blocks until the response (headers and body) has arrived or the
//! timeout has passed, so use it on worker threads only, never on the main
//! thread. Waiting costs no CPU: the caller sleeps on a channel that the
//! request's completion handler fills.

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, Message};
use objc2_foundation::{
    NSData, NSError, NSHTTPCookieAcceptPolicy, NSHTTPURLResponse, NSMutableURLRequest, NSObject, NSObjectProtocol, NSString, NSURLRequest, NSURLRequestCachePolicy, NSURLResponse,
    NSURLSession, NSURLSessionConfiguration, NSURLSessionDelegate, NSURLSessionTask, NSURLSessionTaskDelegate, NSURL,
};
use std::fmt;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Budget for one request: connect + headers + body.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);

/// Bodies larger than this are refused (nothing Poni fetches comes close).
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// How often a download in progress ([`Client::get_bytes`]) says how far it
/// got: only while it runs, for the add-on's progress line.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// URL loading system errors (NSURLError.h).
const URL_ERROR_TIMED_OUT: isize = -1001;
const URL_NETWORK_ERRORS: [isize; 9] = [-1003, -1004, -1005, -1006, -1009, -1018, -1020, -1200, -1206];

/// The methods [`Client::send`] speaks.
#[allow(dead_code)]
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
    /// Seconds the server asked us to wait (`Retry-After`, delta seconds).
    pub retry_after_s: Option<u64>,
}

/// Why a request produced no response.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// Nothing arrived within the timeout; the request was cancelled.
    Timeout,
    /// DNS, connect or TLS failure: no working path to the server.
    Network(String),
    /// Anything else (bad URL, oversized or unreadable body, …).
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

/// The Windows app keeps its worker threads in a COM apartment; the Mac
/// needs nothing of the kind.
pub struct MtaGuard;

impl MtaGuard {
    pub fn new() -> Self {
        MtaGuard
    }
}

impl Default for MtaGuard {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Client {
    user_agent: String,
    /// A session of its own ([`Client::private`]); else the shared one.
    own: Option<Retained<NSURLSession>>,
}

define_class!(
    /// Turns down every redirect, so a 3xx comes back as the answer and no
    /// header goes on to another host (the private session's delegate, on
    /// its own queue).
    #[unsafe(super(NSObject))]
    #[name = "PoniNoRedirect"]
    struct NoRedirect;

    unsafe impl NSObjectProtocol for NoRedirect {}

    unsafe impl NSURLSessionDelegate for NoRedirect {}

    unsafe impl NSURLSessionTaskDelegate for NoRedirect {
        #[unsafe(method(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:))]
        fn will_redirect(&self, _session: &NSURLSession, _task: &NSURLSessionTask, _response: &NSHTTPURLResponse, _request: &NSURLRequest, handler: &DynBlock<dyn Fn(*mut NSURLRequest)>) {
            handler.call((std::ptr::null_mut(),));
        }
    }
);

impl NoRedirect {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

impl Client {
    pub fn new(user_agent: &str) -> Result<Self, FetchError> {
        Ok(Client { user_agent: user_agent.to_string(), own: None })
    }

    /// A client of its own for requests that carry another service's
    /// sign-in (the AI tab's worker): an ephemeral session that keeps no
    /// cookies (a request's own `Cookie` header still goes) and no cache, and
    /// turns down every redirect, so a 3xx comes back as the answer.
    pub fn private(user_agent: &str) -> Result<Self, FetchError> {
        let cfg = NSURLSessionConfiguration::ephemeralSessionConfiguration();
        cfg.setHTTPCookieStorage(None);
        cfg.setHTTPShouldSetCookies(false);
        cfg.setHTTPCookieAcceptPolicy(NSHTTPCookieAcceptPolicy::Never);
        cfg.setURLCache(None);
        cfg.setRequestCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData);
        let delegate = NoRedirect::new();
        let session = unsafe { NSURLSession::sessionWithConfiguration_delegate_delegateQueue(&cfg, Some(ProtocolObject::from_ref(&*delegate)), None) };
        Ok(Client { user_agent: user_agent.to_string(), own: Some(session) })
    }

    /// A client that never answers from a cache and keeps nothing in one,
    /// as Windows' does (the IELTS ladder changes with every session): an
    /// ephemeral session with no cache. Redirects are followed as usual.
    pub fn uncached(user_agent: &str) -> Result<Self, FetchError> {
        let cfg = NSURLSessionConfiguration::ephemeralSessionConfiguration();
        cfg.setURLCache(None);
        cfg.setRequestCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData);
        Ok(Client { user_agent: user_agent.to_string(), own: Some(NSURLSession::sessionWithConfiguration(&cfg)) })
    }

    fn session(&self) -> Retained<NSURLSession> {
        self.own.clone().unwrap_or_else(NSURLSession::sharedSession)
    }

    pub fn get(&self, url: &str, timeout: Duration) -> Result<Response, FetchError> {
        self.send(Method::Get, url, &[], None, timeout)
    }

    pub fn get_with_headers(&self, url: &str, headers: &[(&str, &str)], timeout: Duration) -> Result<Response, FetchError> {
        self.send(Method::Get, url, headers, None, timeout)
    }

    pub fn send(&self, method: Method, url: &str, headers: &[(&str, &str)], json: Option<&str>, timeout: Duration) -> Result<Response, FetchError> {
        let method = match method {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Delete => "DELETE",
        };
        let req = self.request(method, url, headers, timeout)?;
        if let Some(body) = json {
            req.setValue_forHTTPHeaderField(Some(&NSString::from_str("application/json")), &NSString::from_str("Content-Type"));
            req.setHTTPBody(Some(&NSData::with_bytes(body.as_bytes())));
        }
        self.perform(&req, timeout)
    }

    /// POST `body` as it is (a meeting's audio part), with its media type.
    pub fn post_bytes(&self, url: &str, headers: &[(&str, &str)], body: &[u8], content_type: &str, timeout: Duration) -> Result<Response, FetchError> {
        let req = self.request("POST", url, headers, timeout)?;
        req.setValue_forHTTPHeaderField(Some(&NSString::from_str(content_type)), &NSString::from_str("Content-Type"));
        req.setHTTPBody(Some(&NSData::with_bytes(body)));
        self.perform(&req, timeout)
    }

    /// A request to `url` with our User-Agent and `headers`.
    fn request(&self, method: &str, url: &str, headers: &[(&str, &str)], timeout: Duration) -> Result<Retained<NSMutableURLRequest>, FetchError> {
        let nsurl = NSURL::URLWithString(&NSString::from_str(url)).ok_or_else(|| FetchError::Other(format!("not a URL: {url}")))?;
        let req = NSMutableURLRequest::requestWithURL(&nsurl);
        req.setTimeoutInterval(timeout.as_secs_f64());
        req.setHTTPMethod(&NSString::from_str(method));
        req.setValue_forHTTPHeaderField(Some(&NSString::from_str(&self.user_agent)), &NSString::from_str("User-Agent"));
        for (k, v) in headers {
            req.setValue_forHTTPHeaderField(Some(&NSString::from_str(v)), &NSString::from_str(k));
        }
        Ok(req)
    }

    /// Send `req` and wait for its answer.
    fn perform(&self, req: &NSMutableURLRequest, timeout: Duration) -> Result<Response, FetchError> {
        let (tx, rx) = mpsc::channel::<Result<Response, FetchError>>();
        let done = RcBlock::new(move |data: *mut NSData, resp: *mut NSURLResponse, err: *mut NSError| {
            let result = unsafe { finish(data.as_ref(), resp.as_ref(), err.as_ref()) };
            let _ = tx.send(result);
        });
        let task = unsafe { self.session().dataTaskWithRequest_completionHandler(req, &done) };
        task.resume();
        // URLSession enforces the timeout itself; the extra second only guards
        // against a completion handler that never runs.
        match rx.recv_timeout(timeout + Duration::from_secs(1)) {
            Ok(r) => r,
            Err(_) => {
                task.cancel();
                Err(FetchError::Timeout)
            }
        }
    }

    /// GET `url` as raw bytes with its status (an add-on's file): at most
    /// `max_bytes`, all of it within `timeout`. `progress` hears the bytes
    /// received so far while they come in.
    pub fn get_bytes(&self, url: &str, timeout: Duration, max_bytes: u64, mut progress: impl FnMut(u64) + Send + 'static) -> Result<(u16, Vec<u8>), FetchError> {
        let nsurl = NSURL::URLWithString(&NSString::from_str(url)).ok_or_else(|| FetchError::Other(format!("not a URL: {url}")))?;
        let req = NSMutableURLRequest::requestWithURL(&nsurl);
        req.setTimeoutInterval(timeout.as_secs_f64());
        // From the server every time: a file that came back wrong is asked for again.
        req.setCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData);
        req.setValue_forHTTPHeaderField(Some(&NSString::from_str(&self.user_agent)), &NSString::from_str("User-Agent"));
        let (tx, rx) = mpsc::channel::<Result<(u16, Vec<u8>), FetchError>>();
        let done = RcBlock::new(move |data: *mut NSData, resp: *mut NSURLResponse, err: *mut NSError| {
            let result = unsafe { finish_bytes(data.as_ref(), resp.as_ref(), err.as_ref(), max_bytes) };
            let _ = tx.send(result);
        });
        let task = unsafe { self.session().dataTaskWithRequest_completionHandler(&req, &done) };
        task.resume();
        // URLSession's timeout counts a pause in the data, not the whole
        // download, so the budget for all of it is kept here.
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left.min(PROGRESS_EVERY)) {
                Ok(r) => return r,
                Err(RecvTimeoutError::Timeout) if !left.is_zero() => {
                    let got = task.countOfBytesReceived().max(0) as u64;
                    if got > max_bytes {
                        task.cancel();
                        return Err(FetchError::Other(format!("response too large ({got} bytes)")));
                    }
                    progress(got);
                }
                Err(_) => {
                    task.cancel();
                    return Err(FetchError::Timeout);
                }
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // A session of its own keeps its delegate until it is invalidated.
        if let Some(s) = &self.own {
            s.finishTasksAndInvalidate();
        }
    }
}

/// A URLSession error as a [`FetchError`].
fn classify(e: &NSError) -> FetchError {
    let code = e.code();
    let what = e.localizedDescription().to_string();
    if code == URL_ERROR_TIMED_OUT {
        FetchError::Timeout
    } else if URL_NETWORK_ERRORS.contains(&code) {
        FetchError::Network(what)
    } else {
        FetchError::Other(format!("{what} ({code})"))
    }
}

/// Turn a finished data task into a response or an error.
fn finish(data: Option<&NSData>, resp: Option<&NSURLResponse>, err: Option<&NSError>) -> Result<Response, FetchError> {
    if let Some(e) = err {
        return Err(classify(e));
    }
    let http: Option<Retained<NSHTTPURLResponse>> = resp.and_then(|r| r.retain().downcast::<NSHTTPURLResponse>().ok());
    let Some(http) = http else { return Err(FetchError::Other("not an HTTP response".into())) };
    let bytes = data.map(|d| d.to_vec()).unwrap_or_default();
    if bytes.len() > MAX_BODY_BYTES {
        return Err(FetchError::Other(format!("response too large ({} bytes)", bytes.len())));
    }
    let retry_after_s = http.valueForHTTPHeaderField(&NSString::from_str("Retry-After")).and_then(|v| v.to_string().trim().parse::<u64>().ok());
    Ok(Response { status: http.statusCode().clamp(0, u16::MAX as isize) as u16, body: decode_body(&bytes), retry_after_s })
}

/// A finished download ([`Client::get_bytes`]): its status and bytes.
fn finish_bytes(data: Option<&NSData>, resp: Option<&NSURLResponse>, err: Option<&NSError>, max_bytes: u64) -> Result<(u16, Vec<u8>), FetchError> {
    if let Some(e) = err {
        return Err(classify(e));
    }
    let http: Option<Retained<NSHTTPURLResponse>> = resp.and_then(|r| r.retain().downcast::<NSHTTPURLResponse>().ok());
    let Some(http) = http else { return Err(FetchError::Other("not an HTTP response".into())) };
    let len = data.map_or(0, |d| d.len());
    if len as u64 > max_bytes {
        return Err(FetchError::Other(format!("response too large ({len} bytes)")));
    }
    Ok((http.statusCode().clamp(0, u16::MAX as isize) as u16, data.map(|d| d.to_vec()).unwrap_or_default()))
}

/// UTF-8 with a leading BOM dropped; invalid bytes become U+FFFD.
fn decode_body(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(b).into_owned()
}

/// Download raw bytes (album art). Blocking, like the rest.
pub fn get_bytes(url: &str, user_agent: &str, timeout: Duration) -> Result<Vec<u8>, FetchError> {
    let nsurl = NSURL::URLWithString(&NSString::from_str(url)).ok_or_else(|| FetchError::Other(format!("not a URL: {url}")))?;
    let req = NSMutableURLRequest::requestWithURL(&nsurl);
    req.setTimeoutInterval(timeout.as_secs_f64());
    req.setValue_forHTTPHeaderField(Some(&NSString::from_str(user_agent)), &NSString::from_str("User-Agent"));
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, FetchError>>();
    let done = RcBlock::new(move |data: *mut NSData, resp: *mut NSURLResponse, err: *mut NSError| {
        let ok = unsafe { resp.as_ref() }.and_then(|r| r.retain().downcast::<NSHTTPURLResponse>().ok()).is_some_and(|h| h.statusCode() == 200);
        let r = match (unsafe { err.as_ref() }, unsafe { data.as_ref() }) {
            (Some(e), _) => Err(FetchError::Other(e.localizedDescription().to_string())),
            (None, Some(d)) if ok && d.len() <= MAX_BODY_BYTES => Ok(d.to_vec()),
            _ => Err(FetchError::Other("no picture".into())),
        };
        let _ = tx.send(r);
    });
    let task = unsafe { NSURLSession::sharedSession().dataTaskWithRequest_completionHandler(&req, &done) };
    task.resume();
    rx.recv_timeout(timeout + Duration::from_secs(1)).unwrap_or_else(|_| {
        task.cancel();
        Err(FetchError::Timeout)
    })
}
