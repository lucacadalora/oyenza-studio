//! Show over games: the game's frames, counted from Windows' own log of
//! each Present call, the way PresentMon and Xbox Game Bar count them.
//! Nothing is loaded into the game, so there is nothing for anti-cheat to
//! see.
//!
//! A real-time event tracing session listens to the DXGI and Direct3D 9
//! runtimes' Present events and to the graphics kernel's flips, blits and
//! frames handed to the compositor (Vulkan and OpenGL games show up only
//! there). The kernel drops every other event id before it reaches Poni,
//! and the callback keeps only the game's process; `poni_core::frames`
//! turns what's left into a frame rate. The session runs only while the
//! readout shows. Event ids, keywords and payloads follow PresentMon's
//! `PresentData/ETW` headers (MIT).
//!
//! Windows lets admins and the Performance Log Users group run such a
//! session. The one-time OK ([`request_access`]) starts Oyen Poni elevated
//! with `--allow-fps <account SID>`, which adds the account to that group
//! (found by its fixed SID, S-1-5-32-559, so it works in every Windows
//! language) and exits. Windows applies group changes at the next sign-in.

use crate::events::{AppEvent, EventSink, SendHwnd};
use crate::state::FpsAccess;
use poni_core::frames::{FrameCounter, FrameRate, Source};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use windows::core::{w, GUID, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, ERROR_SUCCESS, HANDLE, HLOCAL, WIN32_ERROR,
};
use windows::Win32::NetworkManagement::NetManagement::{
    NetApiBufferFree, NetLocalGroupAddMembers, NetLocalGroupGetMembers, LOCALGROUP_MEMBERS_INFO_0, MAX_PREFERRED_LENGTH,
};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows::Win32::Security::{
    CreateWellKnownSid, EqualSid, GetTokenInformation, LookupAccountSidW, TokenUser, WinBuiltinPerfLoggingUsersSid, PSID, SECURITY_MAX_SID_SIZE,
    SID_NAME_USE, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::System::Diagnostics::Etw::{
    CloseTrace, ControlTraceW, EnableTraceEx2, OpenTraceW, ProcessTrace, CONTROLTRACE_HANDLE, ENABLE_TRACE_PARAMETERS,
    ENABLE_TRACE_PARAMETERS_VERSION_2, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0, EVENT_FILTER_DESCRIPTOR,
    EVENT_FILTER_TYPE_EVENT_ID, EVENT_HEADER_FLAG_32_BIT_HEADER, EVENT_RECORD, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW,
    EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE, PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_RAW_TIMESTAMP,
    PROCESS_TRACE_MODE_REAL_TIME, StartTraceW, TRACE_LEVEL_INFORMATION, WNODE_FLAG_TRACED_GUID,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

/// The one-time OK's command line: `poni.exe --allow-fps <account SID>`.
pub const ALLOW_ARG: &str = "--allow-fps";

const DXGI: GUID = GUID::from_u128(0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9);
const D3D9: GUID = GUID::from_u128(0x783ACA0A_790E_4D7F_8451_AA850511C6B9);
const DXGKRNL: GUID = GUID::from_u128(0x802EC45A_1E99_4B83_9920_87C98277BA9D);
/// The sessions' own ids (the providers' filters are tied to them): the
/// frame count, the self-test's, and the short check of access (Windows
/// picks one for it).
const SESSION_GUID: GUID = GUID::from_u128(0x6f79656e_706f_6e69_8000_6672616d6573);
const SELFTEST_GUID: GUID = GUID::from_u128(0x6f79656e_706f_6e69_8001_6672616d6573);

/// DXGI Present::Start and PresentMultiplaneOverlay::Start; Direct3D 9
/// Present::Start. Both runtimes log them on the game's thread, with the
/// swap chain's address first.
const DXGI_PRESENT: u16 = 0x2a;
const DXGI_PRESENT_MPO: u16 = 0x37;
const D3D9_PRESENT: u16 = 0x01;
/// The runtimes' Present events carry the Analytic channel and Events keywords.
const RUNTIME_KEYWORDS: u64 = 0x8000_0000_0000_0002;
/// The kernel's per-frame events: Flip, FlipMultiPlaneOverlay, Blit,
/// Present, PresentHistory::Start and PresentHistoryDetailed::Start.
const KERNEL_EVENTS: [u16; 6] = [0xa8, 0xfc, 0xa6, 0xb8, 0xab, 0xd7];
/// Base: the one keyword they all carry on Windows 10 and 11 (PresentMon
/// drops their Performance keyword, and their Present keyword before 11).
const KERNEL_KEYWORDS: u64 = 0x1;
/// Room for the session's name (UTF-16, with its NUL).
const NAME_LEN: usize = 64;

/// What the trace's callback shares with the app.
struct Shared {
    /// The game's process; every other process's frames are dropped.
    target: AtomicU32,
    counter: Mutex<FrameCounter>,
    /// Frames counted since the start (the self-test's check).
    total: AtomicU64,
}

/// The running session. Dropping it stops the session; the reading thread
/// then ends by itself (it holds its own share of the counter), so the UI
/// never waits for it.
pub struct Frames {
    shared: Arc<Shared>,
    handle: CONTROLTRACE_HANDLE,
    name: Vec<u16>,
}

#[derive(Debug)]
pub enum StartError {
    /// Not an admin, and not (yet) in Performance Log Users.
    NoAccess,
    Failed(String),
}

/// `EVENT_TRACE_PROPERTIES` with room for the session's name after it.
#[repr(C)]
struct Props {
    p: EVENT_TRACE_PROPERTIES,
    name: [u16; NAME_LEN],
}

impl Props {
    fn new(guid: GUID) -> Box<Props> {
        // SAFETY: plain C data; all zeros is its empty state.
        let mut b: Box<Props> = Box::new(unsafe { std::mem::zeroed() });
        b.p.Wnode.BufferSize = std::mem::size_of::<Props>() as u32;
        b.p.Wnode.Guid = guid;
        // Timestamps from the performance counter: the frames' clock.
        b.p.Wnode.ClientContext = 1;
        b.p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        b.p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        // A second's frames are a few kilobytes: small buffers, flushed every second.
        b.p.BufferSize = 16;
        b.p.MinimumBuffers = 4;
        b.p.MaximumBuffers = 32;
        b.p.FlushTimer = 1;
        b.p.LoggerNameOffset = std::mem::offset_of!(Props, name) as u32;
        b
    }
}

/// "Oyen Poni frames 2": one session per Windows sign-in session, so two
/// people signed in on one PC don't stop each other's.
fn session_name(what: &str) -> Vec<u16> {
    let mut session = 0u32;
    // SAFETY: plain out parameter.
    let _ = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    let mut v: Vec<u16> = format!("Oyen Poni {what} {session}").encode_utf16().collect();
    v.truncate(NAME_LEN - 1);
    v.push(0);
    v
}

fn start_session(name: &[u16], guid: GUID) -> Result<CONTROLTRACE_HANDLE, WIN32_ERROR> {
    let mut handle = CONTROLTRACE_HANDLE::default();
    let mut props = Props::new(guid);
    // SAFETY: `name` is NUL-terminated; `props` has room for the name it writes back.
    let mut err = unsafe { StartTraceW(&mut handle, PCWSTR(name.as_ptr()), &mut props.p) };
    // Left running by a Poni that didn't get to stop it (a crash): stop and start again.
    if err == ERROR_ALREADY_EXISTS {
        stop_session(CONTROLTRACE_HANDLE::default(), name);
        props = Props::new(guid);
        err = unsafe { StartTraceW(&mut handle, PCWSTR(name.as_ptr()), &mut props.p) };
    }
    if err == ERROR_SUCCESS {
        Ok(handle)
    } else {
        Err(err)
    }
}

fn stop_session(handle: CONTROLTRACE_HANDLE, name: &[u16]) {
    let mut props = Props::new(GUID::zeroed());
    let name = if handle.Value == 0 { PCWSTR(name.as_ptr()) } else { PCWSTR::null() };
    // SAFETY: `props` has room for the name the call writes back.
    let _ = unsafe { ControlTraceW(handle, name, &mut props.p, EVENT_TRACE_CONTROL_STOP) };
}

/// Turns one provider on for the session `session`, for these event ids only.
fn enable(handle: CONTROLTRACE_HANDLE, session: GUID, provider: &GUID, keywords: u64, ids: &[u16]) -> WIN32_ERROR {
    // EVENT_FILTER_EVENT_ID: FilterIn, Reserved, Count, then the ids.
    let mut filter: Vec<u16> = Vec::with_capacity(2 + ids.len());
    filter.push(1);
    filter.push(ids.len() as u16);
    filter.extend_from_slice(ids);
    let mut desc = EVENT_FILTER_DESCRIPTOR { Ptr: filter.as_ptr() as u64, Size: (filter.len() * 2) as u32, Type: EVENT_FILTER_TYPE_EVENT_ID };
    let params = ENABLE_TRACE_PARAMETERS {
        Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
        EnableProperty: EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0,
        ControlFlags: 0,
        SourceId: session,
        EnableFilterDesc: &mut desc,
        FilterDescCount: 1,
    };
    // SAFETY: `filter` and `desc` outlive the call.
    unsafe {
        EnableTraceEx2(
            handle,
            provider,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER.0,
            TRACE_LEVEL_INFORMATION as u8,
            keywords,
            keywords,
            0,
            Some(&params as *const ENABLE_TRACE_PARAMETERS),
        )
    }
}

fn qpc_now() -> i64 {
    let mut t = 0i64;
    // SAFETY: plain out parameter.
    let _ = unsafe { QueryPerformanceCounter(&mut t) };
    t
}

impl Frames {
    /// Starts counting the frames of process `pid`.
    pub fn start(pid: u32) -> Result<Frames, StartError> {
        Self::start_named(pid, session_name("frames"), SESSION_GUID)
    }

    /// The self-test's own session, so it never stops a running Poni's.
    pub fn start_selftest(pid: u32) -> Result<Frames, StartError> {
        Self::start_named(pid, session_name("frames self-test"), SELFTEST_GUID)
    }

    fn start_named(pid: u32, name: Vec<u16>, guid: GUID) -> Result<Frames, StartError> {
        let handle = match start_session(&name, guid) {
            Ok(h) => h,
            Err(e) if e == ERROR_ACCESS_DENIED => return Err(StartError::NoAccess),
            Err(e) => return Err(StartError::Failed(format!("Windows error {}", e.0))),
        };
        let mut freq = 0i64;
        // SAFETY: plain out parameter.
        let _ = unsafe { QueryPerformanceFrequency(&mut freq) };
        let shared = Arc::new(Shared { target: AtomicU32::new(pid), counter: Mutex::new(FrameCounter::new(freq)), total: AtomicU64::new(0) });
        // From here on, dropping `frames` stops the session.
        let frames = Frames { shared: shared.clone(), handle, name: name.clone() };
        // The runtimes come first: a game's own Present calls are what counts.
        for (provider, keywords, ids) in [
            (&DXGI, RUNTIME_KEYWORDS, &[DXGI_PRESENT, DXGI_PRESENT_MPO][..]),
            (&D3D9, RUNTIME_KEYWORDS, &[D3D9_PRESENT][..]),
            (&DXGKRNL, KERNEL_KEYWORDS, &KERNEL_EVENTS[..]),
        ] {
            let e = enable(handle, guid, provider, keywords, ids);
            if e == ERROR_ACCESS_DENIED {
                return Err(StartError::NoAccess);
            }
            if e != ERROR_SUCCESS {
                // One provider missing still counts the others' games.
                crate::pwarn!("frames: provider {provider:?} not enabled ({})", e.0);
            }
        }
        std::thread::Builder::new().name("poni-frames".into()).spawn(move || consume(name, shared)).map_err(|e| StartError::Failed(e.to_string()))?;
        Ok(frames)
    }

    /// Follows another process (the player switched games).
    pub fn set_target(&self, pid: u32) {
        if self.shared.target.swap(pid, Ordering::Relaxed) != pid {
            if let Ok(mut c) = self.shared.counter.lock() {
                c.clear();
            }
        }
    }

    /// The game's frame rate now (None while it doesn't draw).
    pub fn rate(&self) -> Option<FrameRate> {
        self.shared.counter.lock().ok()?.rate(qpc_now())
    }

    /// Frames counted since the start.
    pub fn total(&self) -> u64 {
        self.shared.total.load(Ordering::Relaxed)
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        // Ends ProcessTrace on the reading thread.
        stop_session(self.handle, &self.name);
    }
}

/// The consumer thread: delivers the session's events to [`on_event`]
/// until the session stops.
fn consume(mut name: Vec<u16>, shared: Arc<Shared>) {
    let mut log = EVENT_TRACE_LOGFILEW { LoggerName: PWSTR(name.as_mut_ptr()), Context: Arc::as_ptr(&shared) as *mut c_void, ..Default::default() };
    log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    log.Anonymous2.EventRecordCallback = Some(on_event);
    // SAFETY: `log`, the name and `shared` (held by this thread) outlive the trace.
    let trace = unsafe { OpenTraceW(&mut log) };
    if trace.Value == u64::MAX {
        crate::pwarn!("frames: couldn't open the trace ({})", windows::core::Error::from_win32());
        return;
    }
    let e = unsafe { ProcessTrace(&[trace], None, None) };
    if e != ERROR_SUCCESS && e != ERROR_CANCELLED {
        crate::pwarn!("frames: trace ended ({})", e.0);
    }
    unsafe {
        let _ = CloseTrace(trace);
    }
}

/// One event: a frame of the game's, or something to drop.
unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    // SAFETY: ETW passes a valid record for the duration of the call, with
    // the context `consume` gave it (alive until ProcessTrace returns).
    let Some(rec) = (unsafe { rec.as_ref() }) else { return };
    let Some(shared) = (unsafe { (rec.UserContext as *const Shared).as_ref() }) else { return };
    let h = &rec.EventHeader;
    if h.ProcessId != shared.target.load(Ordering::Relaxed) {
        return;
    }
    let id = h.EventDescriptor.Id;
    let source = if h.ProviderId == DXGI && (id == DXGI_PRESENT || id == DXGI_PRESENT_MPO) {
        Source::Dxgi(first_pointer(rec))
    } else if h.ProviderId == D3D9 && id == D3D9_PRESENT {
        Source::D3d9(first_pointer(rec))
    } else if h.ProviderId == DXGKRNL && KERNEL_EVENTS.contains(&id) {
        Source::Kernel(id)
    } else {
        return;
    };
    shared.total.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut c) = shared.counter.lock() {
        c.push(source, h.TimeStamp);
    }
}

/// The payload's first field, a pointer (32-bit in a 32-bit game).
fn first_pointer(rec: &EVENT_RECORD) -> u64 {
    let narrow = u32::from(rec.EventHeader.Flags) & EVENT_HEADER_FLAG_32_BIT_HEADER != 0;
    let size = if narrow { 4 } else { 8 };
    let data = rec.UserData as *const u8;
    if data.is_null() || usize::from(rec.UserDataLength) < size {
        return 0;
    }
    // SAFETY: the payload holds at least `size` bytes; it needn't be aligned.
    unsafe {
        if narrow {
            u64::from(std::ptr::read_unaligned(data as *const u32))
        } else {
            std::ptr::read_unaligned(data as *const u64)
        }
    }
}

/// Stops a frame count that a Poni which crashed left running in this
/// sign-in session (the next one would stop it anyway; this stops it now).
pub fn stop_stale() {
    stop_session(CONTROLTRACE_HANDLE::default(), &session_name("frames"));
}

// ---------------------------------------------------------------- access

/// Whether this sign-in may read frames: a short-lived session tells.
/// Without access, an account that's already in the group (the one-time
/// OK went through) only waits for the next sign-in.
pub fn check() -> FpsAccess {
    let name = session_name("frames check");
    match start_session(&name, GUID::zeroed()) {
        Ok(h) => {
            stop_session(h, &name);
            FpsAccess::Ready
        }
        Err(e) if e == ERROR_ACCESS_DENIED => {
            if account_in_group() {
                FpsAccess::SignOut
            } else {
                FpsAccess::NeedsOk
            }
        }
        Err(e) => FpsAccess::Failed(format!("Windows error {}", e.0)),
    }
}

/// [`check`] off the UI thread; the answer arrives as `AppEvent::FpsAccess`.
pub fn check_async(sink: EventSink) {
    let _ = std::thread::Builder::new().name("poni-fps-check".into()).spawn(move || sink.send(AppEvent::FpsAccess(check())));
}

/// The one-time OK: Windows' admin prompt for `poni.exe --allow-fps <SID>`,
/// then the answer as `AppEvent::FpsAccess` (SignOut once the account is in
/// the group, NeedsOk when the prompt was declined). `owner` is the island,
/// in front for the prompt (Windows shows a background app's prompt only as
/// a flashing taskbar button).
pub fn request_access(sink: EventSink, owner: SendHwnd) {
    let _ = std::thread::Builder::new().name("poni-fps-allow".into()).spawn(move || {
        let answer = match ask_windows(owner) {
            Ok(0) => {
                // An admin running Poni elevated may read frames at once.
                match check() {
                    FpsAccess::Ready => FpsAccess::Ready,
                    _ => FpsAccess::SignOut,
                }
            }
            Ok(code) => FpsAccess::Failed(format!("Windows error {code}")),
            Err(Declined) => FpsAccess::NeedsOk,
            Err(Failed(why)) => FpsAccess::Failed(why),
        };
        crate::pinfo!("frames: the one-time OK ended: {answer:?}");
        sink.send(AppEvent::FpsAccess(answer));
    });
}

enum AskError {
    Declined,
    Failed(String),
}
use AskError::{Declined, Failed};

/// Runs this exe elevated with the account's SID and waits for its exit code.
fn ask_windows(owner: SendHwnd) -> Result<u32, AskError> {
    let sid = account_sid_string().ok_or_else(|| Failed("couldn't read the account".into()))?;
    let exe = std::env::current_exe().map_err(|e| Failed(e.to_string()))?;
    let file = HSTRING::from(exe.as_os_str());
    let args = HSTRING::from(format!("{ALLOW_ARG} {sid}"));
    // SAFETY: COM for ShellExecuteEx's sake (it may hand the verb to a shell extension).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        hwnd: owner.hwnd(),
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() { Declined } else { Failed(e.message()) });
    }
    if info.hProcess.is_invalid() {
        return Err(Failed("no process".into()));
    }
    let mut code = 1u32;
    // SAFETY: a process handle we own.
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(code)
}

/// The signed-in account's SID ("S-1-5-21-…").
fn account_sid_string() -> Option<String> {
    let token = token_user()?;
    // SAFETY: TOKEN_USER starts the buffer; its SID points inside it.
    let user = unsafe { (*token.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut s = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user, &mut s) }.ok()?;
    // SAFETY: the call allocated `s` with LocalAlloc.
    let out = unsafe { s.to_string() }.ok();
    unsafe {
        let _ = LocalFree(HLOCAL(s.0.cast()));
    }
    out
}

/// This process's TOKEN_USER (u64 keeps it aligned).
fn token_user() -> Option<Vec<u64>> {
    let mut token = HANDLE::default();
    // SAFETY: plain handle out parameter, closed below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    let mut len = 0u32;
    unsafe {
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
    let ok = unsafe { GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len) }.is_ok();
    unsafe {
        let _ = CloseHandle(token);
    }
    ok.then_some(buf)
}

/// Performance Log Users, by its fixed SID, and its name in this Windows' language.
fn perf_log_users() -> Option<(Vec<u8>, Vec<u16>)> {
    let mut sid = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut len = sid.len() as u32;
    // SAFETY: `sid` has room for any SID.
    unsafe { CreateWellKnownSid(WinBuiltinPerfLoggingUsersSid, PSID::default(), PSID(sid.as_mut_ptr().cast()), &mut len) }.ok()?;
    let mut name = vec![0u16; 256];
    let mut domain = vec![0u16; 256];
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut kind = SID_NAME_USE::default();
    unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            PSID(sid.as_mut_ptr().cast()),
            PWSTR(name.as_mut_ptr()),
            &mut name_len,
            PWSTR(domain.as_mut_ptr()),
            &mut domain_len,
            &mut kind,
        )
    }
    .ok()?;
    name.truncate(name_len as usize);
    name.push(0);
    Some((sid, name))
}

/// The account is in Performance Log Users (whatever its sign-in token says).
fn account_in_group() -> bool {
    let (Some(token), Some((_, group))) = (token_user(), perf_log_users()) else { return false };
    // SAFETY: TOKEN_USER starts the buffer.
    let user = unsafe { (*token.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut buf: *mut u8 = std::ptr::null_mut();
    let (mut read, mut total) = (0u32, 0u32);
    let err = unsafe { NetLocalGroupGetMembers(PCWSTR::null(), PCWSTR(group.as_ptr()), 0, &mut buf, MAX_PREFERRED_LENGTH, &mut read, &mut total, None) };
    if err != 0 || buf.is_null() {
        return false;
    }
    // SAFETY: level 0 fills `read` LOCALGROUP_MEMBERS_INFO_0 entries.
    let members = unsafe { std::slice::from_raw_parts(buf.cast::<LOCALGROUP_MEMBERS_INFO_0>(), read as usize) };
    let found = members.iter().any(|m| unsafe { EqualSid(m.lgrmi0_sid, user) }.is_ok());
    unsafe {
        NetApiBufferFree(Some(buf.cast()));
    }
    found
}

/// `poni.exe --allow-fps <SID>`, elevated: adds that account to
/// Performance Log Users. Exit code 0 when it's in the group (already
/// being there counts), else Windows' error.
pub fn allow_main(sid: &str) -> i32 {
    let Some((_, group)) = perf_log_users() else { return 1 };
    let mut user = PSID::default();
    if unsafe { ConvertStringSidToSidW(&HSTRING::from(sid), &mut user) }.is_err() {
        return 87; // ERROR_INVALID_PARAMETER
    }
    let member = LOCALGROUP_MEMBERS_INFO_0 { lgrmi0_sid: user };
    // SAFETY: one level-0 entry; the SID stays alive until LocalFree.
    let err = unsafe { NetLocalGroupAddMembers(PCWSTR::null(), PCWSTR(group.as_ptr()), 0, (&member as *const LOCALGROUP_MEMBERS_INFO_0).cast(), 1) };
    unsafe {
        let _ = LocalFree(HLOCAL(user.0));
    }
    // ERROR_MEMBER_IN_ALIAS: already in the group.
    if err == 0 || err == 1378 {
        0
    } else {
        err as i32
    }
}
