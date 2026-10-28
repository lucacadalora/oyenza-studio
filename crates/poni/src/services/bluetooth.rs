//! Bluetooth connect/disconnect pop-ups with battery level.
//!
//! Two WinRT `DeviceWatcher`s (classic and LE association endpoints, filtered
//! to devices that are paired with this PC and connected) feed one worker
//! thread. The watcher handlers run on the thread pool and only copy a few
//! properties into a message. The worker:
//!
//! * records the devices found during the initial enumeration silently
//!   (already-connected devices are not news),
//! * groups endpoints by device container, so a dual-mode headset (classic +
//!   LE endpoint) is one device and pops up once,
//! * waits until a change has held for 3 s: a device that connects for a
//!   moment and drops again (or the other way round) shows nothing,
//! * pops up only for headphones, earbuds and speakers (and devices that
//!   don't say what they are): phones, mice, keyboards and watches connect
//!   and drop by themselves all day, so they stay quiet,
//! * looks up the battery level (PnP property, then GATT Battery Service for
//!   LE) with bounded waits, so a slow device delays its pop-up by at most
//!   ~2.5 s and a failed lookup just means no battery badge.
//!
//! Idle cost: the worker sleeps in a blocking `recv` (with a timeout only
//! while a change is settling or a watcher restart is due); nothing polls.

use crate::events::{AppEvent, EventSink};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{IInspectable, Interface, Result, GUID, HSTRING};
use windows::Devices::Bluetooth::GenericAttributeProfile::{GattCharacteristicUuids, GattCommunicationStatus, GattDeviceService, GattServiceUuids};
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice, BluetoothMajorClass};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher, DeviceWatcherStatus};
use windows::Foundation::Collections::{IIterable, IMapView};
use windows::Foundation::{AsyncOperationCompletedHandler, AsyncStatus, EventRegistrationToken, IAsyncOperation, IPropertyValue, PropertyType, TypedEventHandler};
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// A device's change pops up once it has held this long; changes closer
/// together than this end up as one report, or none if the device ends up
/// where it started.
const SETTLE: Duration = Duration::from_secs(3);
/// Upper bound for all lookups behind one report.
const LOOKUP_BUDGET: Duration = Duration::from_millis(2500);
/// Opening a device object (local, normally a few ms).
const OPEN_TIMEOUT: Duration = Duration::from_millis(1000);
/// PnP battery property query over the device's container.
const PNP_TIMEOUT: Duration = Duration::from_millis(1500);
/// GATT Battery Service read (radio round trips).
const GATT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long `Drop` waits for the worker before detaching it.
const STOP_TIMEOUT: Duration = Duration::from_millis(1000);
/// Restarts of an aborted watcher (delay grows linearly).
const MAX_RESTARTS: u32 = 5;
const RESTART_DELAY: Duration = Duration::from_secs(5);

/// DEVPKEY_Bluetooth_Battery (byte, 0..=100), set on the device's PnP nodes.
const PKEY_BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
const PKEY_CONTAINER: &str = "System.Devices.Aep.ContainerId";
const PKEY_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const PKEY_COD_MAJOR: &str = "System.Devices.Aep.Bluetooth.Cod.Major";
const PKEY_LE_CATEGORY: &str = "System.Devices.Aep.Bluetooth.Le.Appearance.Category";

/// Added to Windows' own "connected" filters. Something that was never paired
/// with this PC (a phone or laptop nearby, a tracker) can turn up as a
/// connected Bluetooth LE device for as long as it talks to the PC, often
/// with no name: a "Bluetooth device" that connected and disconnected while
/// the person had connected nothing.
const AQS_PAIRED: &str = "System.Devices.Aep.IsPaired:=System.StructuredQueryType.Boolean#True";

/// Bluetooth LE appearance categories (appearance >> 6) that are audio gear:
/// media player, audio sink, audio source, AV equipment, wearable audio
/// device (earbuds / headsets / headphones), hearing aid.
const LE_AUDIO_CATEGORIES: [u32; 6] = [0x0A, 0x21, 0x22, 0x25, 0x27, 0x29];

const FALLBACK_NAME: &str = "Bluetooth device";

/// Bluetooth connect/disconnect watcher. Dropping it stops the watchers.
pub struct BluetoothService {
    tx: Sender<Msg>,
    done: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl BluetoothService {
    pub fn start(sink: EventSink) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let worker_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("poni-bluetooth".into())
            .spawn(move || {
                run(sink, worker_tx, rx);
                let _ = done_tx.send(());
            })
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("bluetooth thread: {e}")))?;
        Ok(Self { tx, done, thread: Some(thread) })
    }
}

impl Drop for BluetoothService {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        let Some(thread) = self.thread.take() else { return };
        match self.done.recv_timeout(STOP_TIMEOUT) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
            }
            // Most likely inside a bounded battery lookup; the process is
            // exiting anyway, so don't hold the UI thread hostage.
            Err(RecvTimeoutError::Timeout) => crate::pwarn!("bluetooth: worker did not stop in time; detaching it"),
        }
    }
}

// ---------------------------------------------------------------------------
// Messages from the watcher handlers.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Classic,
    Le,
}

impl Kind {
    const ALL: [Kind; 2] = [Kind::Classic, Kind::Le];

    fn index(self) -> usize {
        match self {
            Kind::Classic => 0,
            Kind::Le => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Classic => "classic",
            Kind::Le => "LE",
        }
    }

    /// Windows' own filter for connected devices of this kind.
    fn connected_selector(self) -> Result<HSTRING> {
        match self {
            Kind::Classic => BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
            Kind::Le => BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
        }
    }

    /// The watcher's filter: connected and paired with this PC.
    fn selector(self) -> Result<HSTRING> {
        Ok(HSTRING::from(paired_only(&self.connected_selector()?.to_string_lossy())))
    }

    /// The property that hints whether the device is audio gear.
    fn class_key(self) -> &'static str {
        match self {
            Kind::Classic => PKEY_COD_MAJOR,
            Kind::Le => PKEY_LE_CATEGORY,
        }
    }

    /// Extra watcher properties, from most to least ambitious (older builds
    /// may reject a property name; then we retry with fewer).
    fn property_sets(self) -> [&'static [&'static str]; 3] {
        match self {
            Kind::Classic => [&[PKEY_CONTAINER, PKEY_CONNECTED, PKEY_COD_MAJOR], &[PKEY_CONTAINER, PKEY_CONNECTED], &[]],
            Kind::Le => [&[PKEY_CONTAINER, PKEY_CONNECTED, PKEY_LE_CATEGORY], &[PKEY_CONTAINER, PKEY_CONNECTED], &[]],
        }
    }
}

/// An endpoint reported by a watcher.
struct Seen {
    kind: Kind,
    id: String,
    name: String,
    container: Option<GUID>,
    /// CoD major class (classic) or appearance category (LE).
    class_hint: Option<u32>,
    at: Instant,
}

impl Seen {
    fn read(kind: Kind, info: &DeviceInformation) -> Option<Seen> {
        let id = info.Id().ok()?.to_string_lossy();
        let name = info.Name().map(|n| n.to_string_lossy().trim().to_string()).unwrap_or_default();
        let props = info.Properties().ok();
        let prop_u = |key: &str| props.as_ref().and_then(|p| prop_u32(p, key));
        if props.as_ref().and_then(|p| prop_bool(p, PKEY_CONNECTED)) == Some(false) {
            return None;
        }
        Some(Seen {
            kind,
            id,
            name,
            container: props.as_ref().and_then(|p| prop_guid(p, PKEY_CONTAINER)).filter(usable_container),
            class_hint: prop_u(kind.class_key()).filter(|v| *v != 0),
            at: Instant::now(),
        })
    }
}

enum Msg {
    Added { gen: u32, seen: Seen },
    /// A known endpoint (dis)connected (watcher removal or IsConnected update).
    Changed { gen: u32, kind: Kind, id: String, connected: bool, at: Instant },
    Enumerated { gen: u32, kind: Kind },
    Stopped { gen: u32, kind: Kind, aborted: bool },
    Shutdown,
}

// ---------------------------------------------------------------------------
// Watchers.

#[derive(Default)]
struct Tokens {
    added: Option<EventRegistrationToken>,
    updated: Option<EventRegistrationToken>,
    removed: Option<EventRegistrationToken>,
    completed: Option<EventRegistrationToken>,
    stopped: Option<EventRegistrationToken>,
}

struct Watch {
    watcher: DeviceWatcher,
    tokens: Tokens,
}

impl Watch {
    fn start(kind: Kind, aqs: &HSTRING, level: usize, gen: u32, tx: &Sender<Msg>) -> Result<Watch> {
        let props = kind.property_sets()[level.min(2)];
        let watcher = if props.is_empty() {
            DeviceInformation::CreateWatcherAqsFilter(aqs)?
        } else {
            let list = IIterable::<HSTRING>::try_from(props.iter().map(|p| HSTRING::from(*p)).collect::<Vec<_>>())?;
            DeviceInformation::CreateWatcherAqsFilterAndAdditionalProperties(aqs, &list)?
        };
        let mut w = Watch { watcher, tokens: Tokens::default() };
        if let Err(e) = w.subscribe(kind, gen, tx).and_then(|()| w.watcher.Start()) {
            w.stop();
            return Err(e);
        }
        Ok(w)
    }

    fn subscribe(&mut self, kind: Kind, gen: u32, tx: &Sender<Msg>) -> Result<()> {
        let t = tx.clone();
        self.tokens.added = Some(self.watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(move |_, info| {
            if let Some(seen) = info.as_ref().and_then(|i| Seen::read(kind, i)) {
                let _ = t.send(Msg::Added { gen, seen });
            }
            Ok(())
        }))?);

        let t = tx.clone();
        self.tokens.removed = Some(self.watcher.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, upd| {
            if let Some(id) = upd.as_ref().and_then(|u| u.Id().ok()) {
                let _ = t.send(Msg::Changed { gen, kind, id: id.to_string_lossy(), connected: false, at: Instant::now() });
            }
            Ok(())
        }))?);

        // Belt and braces: some stacks report (dis)connects as an update of
        // IsConnected instead of removing / re-adding the endpoint.
        let t = tx.clone();
        self.tokens.updated = Some(self.watcher.Updated(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, upd| {
            if let Some(u) = upd {
                if let Some(connected) = u.Properties().ok().and_then(|p| prop_bool(&p, PKEY_CONNECTED)) {
                    if let Ok(id) = u.Id() {
                        let _ = t.send(Msg::Changed { gen, kind, id: id.to_string_lossy(), connected, at: Instant::now() });
                    }
                }
            }
            Ok(())
        }))?);

        let t = tx.clone();
        self.tokens.completed = Some(self.watcher.EnumerationCompleted(&TypedEventHandler::<DeviceWatcher, IInspectable>::new(move |_, _| {
            let _ = t.send(Msg::Enumerated { gen, kind });
            Ok(())
        }))?);

        let t = tx.clone();
        self.tokens.stopped = Some(self.watcher.Stopped(&TypedEventHandler::<DeviceWatcher, IInspectable>::new(move |w, _| {
            let aborted = w.as_ref().and_then(|w| w.Status().ok()) == Some(DeviceWatcherStatus::Aborted);
            let _ = t.send(Msg::Stopped { gen, kind, aborted });
            Ok(())
        }))?);
        Ok(())
    }

    /// Stop (if running) and release the handlers (and the senders they hold).
    fn stop(self) {
        let status = self.watcher.Status().ok();
        if status == Some(DeviceWatcherStatus::Started) || status == Some(DeviceWatcherStatus::EnumerationCompleted) {
            if let Err(e) = self.watcher.Stop() {
                crate::pwarn!("bluetooth: stopping a watcher failed ({e})");
            }
        }
        let w = &self.watcher;
        if let Some(t) = self.tokens.added {
            let _ = w.RemoveAdded(t);
        }
        if let Some(t) = self.tokens.updated {
            let _ = w.RemoveUpdated(t);
        }
        if let Some(t) = self.tokens.removed {
            let _ = w.RemoveRemoved(t);
        }
        if let Some(t) = self.tokens.completed {
            let _ = w.RemoveEnumerationCompleted(t);
        }
        if let Some(t) = self.tokens.stopped {
            let _ = w.RemoveStopped(t);
        }
    }
}

// ---------------------------------------------------------------------------
// Worker state.

#[derive(Clone)]
struct Endpoint {
    id: String,
    kind: Kind,
    connected: bool,
    class_hint: Option<u32>,
}

struct Device {
    name: String,
    container: Option<GUID>,
    endpoints: Vec<Endpoint>,
    /// Resolved once per device and remembered (also for the disconnect).
    audio: Option<bool>,
    /// Connection state last reported (or learned silently); a device never
    /// reported counts as disconnected.
    shown: bool,
    /// A change is settling; it pops up at this time if it still holds.
    due: Option<Instant>,
}

impl Device {
    fn connected(&self) -> bool {
        self.endpoints.iter().any(|e| e.connected)
    }

    /// The device's connection changed at `at`. During the watcher's first
    /// listing that is only noted; otherwise the change pops up once it has
    /// held for `SETTLE` (every further change starts the wait again).
    fn changed(&mut self, at: Instant, silent: bool) {
        if silent {
            self.shown = self.connected();
            self.due = None;
        } else {
            self.due = Some(at + SETTLE);
        }
    }

    /// The state to report at `now`, once a change has settled: `None` while
    /// it is still settling, or when the device is back where it was shown.
    fn settled(&mut self, now: Instant) -> Option<bool> {
        if !self.due.is_some_and(|t| t <= now) {
            return None;
        }
        self.due = None;
        let connected = self.connected();
        (connected != self.shown).then_some(connected)
    }
}

/// What the lookups need, detached from the worker's maps.
struct Snapshot {
    container: Option<GUID>,
    endpoints: Vec<Endpoint>,
    audio: Option<bool>,
}

impl Snapshot {
    /// An endpoint of `kind`, preferring a connected one.
    fn endpoint(&self, kind: Kind) -> Option<&Endpoint> {
        let mut of_kind = self.endpoints.iter().filter(|e| e.kind == kind);
        let first = of_kind.clone().next();
        of_kind.find(|e| e.connected).or(first)
    }
}

#[derive(Default)]
struct Slot {
    watch: Option<Watch>,
    generation: u32,
    enumerated: bool,
    /// Index into `Kind::property_sets`.
    level: usize,
    restarts: u32,
    restart_at: Option<Instant>,
}

struct Worker {
    sink: EventSink,
    tx: Sender<Msg>,
    devices: HashMap<String, Device>,
    /// Endpoint id -> device key.
    keys: HashMap<String, String>,
    slots: [Slot; 2],
}

/// COM apartment membership for the current thread.
struct ComApartment {
    initialized: bool,
}

impl ComApartment {
    fn enter_mta() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            crate::pwarn!("bluetooth: CoInitializeEx failed ({hr:?})");
        }
        Self { initialized: hr.is_ok() }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}

fn run(sink: EventSink, tx: Sender<Msg>, rx: Receiver<Msg>) {
    // Declared first so it is dropped last, after every WinRT object.
    let com = ComApartment::enter_mta();
    if !com.initialized {
        return;
    }
    let mut w = Worker { sink, tx, devices: HashMap::new(), keys: HashMap::new(), slots: Default::default() };
    let started = Kind::ALL.iter().filter(|k| w.start_watch(**k)).count();
    if started == 0 {
        crate::pwarn!("bluetooth: no device watcher could start; pop-ups disabled");
        return;
    }

    loop {
        let msg = match w.next_deadline() {
            Some(due) => match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };
        match msg {
            Some(Msg::Shutdown) => break,
            Some(m) => w.handle(m),
            None => {}
        }
        w.tick(Instant::now());
    }
    w.stop_all();
}

impl Worker {
    fn start_watch(&mut self, kind: Kind) -> bool {
        let slot = &mut self.slots[kind.index()];
        slot.generation = slot.generation.wrapping_add(1);
        slot.enumerated = false;
        let aqs = match kind.selector() {
            Ok(aqs) => aqs,
            Err(e) => {
                crate::pwarn!("bluetooth: no {} filter ({e})", kind.label());
                return false;
            }
        };
        for level in slot.level..3 {
            match Watch::start(kind, &aqs, level, slot.generation, &self.tx) {
                Ok(watch) => {
                    slot.watch = Some(watch);
                    slot.level = level;
                    return true;
                }
                Err(e) => crate::pwarn!("bluetooth: {} watcher (property set {level}) failed to start ({e})", kind.label()),
            }
        }
        false
    }

    fn stop_all(&mut self) {
        for slot in &mut self.slots {
            if let Some(watch) = slot.watch.take() {
                watch.stop();
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let devices = self.devices.values().filter_map(|d| d.due);
        let restarts = self.slots.iter().filter_map(|s| s.restart_at);
        devices.chain(restarts).min()
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Added { gen, seen } => {
                let Some(silent) = self.live(seen.kind, gen) else { return };
                self.on_added(seen, silent);
            }
            Msg::Changed { gen, kind, id, connected, at } => {
                let Some(silent) = self.live(kind, gen) else { return };
                self.on_changed(&id, connected, at, silent);
            }
            Msg::Enumerated { gen, kind } => {
                if self.live(kind, gen).is_some() {
                    self.slots[kind.index()].enumerated = true;
                    crate::pinfo!("bluetooth: {} watcher ready", kind.label());
                }
            }
            Msg::Stopped { gen, kind, aborted } => {
                if self.live(kind, gen).is_some() {
                    self.on_stopped(kind, aborted);
                }
            }
            Msg::Shutdown => {}
        }
    }

    /// `Some(silent)` when the message comes from the current watcher of
    /// `kind`; `silent` while its initial enumeration is still running.
    fn live(&self, kind: Kind, gen: u32) -> Option<bool> {
        let slot = &self.slots[kind.index()];
        (slot.watch.is_some() && slot.generation == gen).then_some(!slot.enumerated)
    }

    fn on_added(&mut self, seen: Seen, silent: bool) {
        let key = match self.keys.get(&seen.id) {
            Some(k) => k.clone(),
            None => match &seen.container {
                Some(c) => format!("{c:?}"),
                None => format!("aep:{}", seen.id),
            },
        };
        let dev = self.devices.entry(key.clone()).or_insert_with(|| Device {
            name: String::new(),
            container: None,
            endpoints: Vec::new(),
            audio: None,
            shown: false,
            due: None,
        });
        let was = dev.connected();
        // Classic endpoints carry the user-facing name; LE names are a fallback.
        if !seen.name.is_empty() && (dev.name.is_empty() || seen.kind == Kind::Classic) {
            dev.name = seen.name;
        }
        dev.container = dev.container.or(seen.container);
        match dev.endpoints.iter_mut().find(|e| e.id == seen.id) {
            Some(e) => {
                e.connected = true;
                e.class_hint = seen.class_hint.or(e.class_hint);
            }
            None => dev.endpoints.push(Endpoint { id: seen.id.clone(), kind: seen.kind, connected: true, class_hint: seen.class_hint }),
        }
        self.keys.insert(seen.id, key.clone());
        self.transition(&key, was, seen.at, silent);
    }

    /// Only for endpoints we already know (an unknown one that connects
    /// arrives as `Added`, with its name).
    fn on_changed(&mut self, id: &str, connected: bool, at: Instant, silent: bool) {
        let Some(key) = self.keys.get(id).cloned() else { return };
        let Some(dev) = self.devices.get_mut(&key) else { return };
        let was = dev.connected();
        let Some(e) = dev.endpoints.iter_mut().find(|e| e.id == id) else { return };
        e.connected = connected;
        self.transition(&key, was, at, silent);
    }

    fn on_stopped(&mut self, kind: Kind, aborted: bool) {
        let slot = &mut self.slots[kind.index()];
        if let Some(watch) = slot.watch.take() {
            watch.stop();
        }
        if !slot.enumerated && slot.level < 2 {
            // It died before finishing; maybe a requested property is the
            // problem on this build. Ask for less next time.
            slot.level += 1;
        }
        slot.enumerated = false;
        crate::pwarn!("bluetooth: {} watcher {}", kind.label(), if aborted { "aborted" } else { "stopped" });

        // Its endpoints are unknown now; the restart re-learns the connected
        // ones silently.
        for dev in self.devices.values_mut() {
            let mut changed = false;
            for e in dev.endpoints.iter_mut().filter(|e| e.kind == kind && e.connected) {
                e.connected = false;
                changed = true;
            }
            if changed {
                dev.shown = dev.connected();
                dev.due = None;
            }
        }

        self.schedule_restart(kind);
    }

    fn schedule_restart(&mut self, kind: Kind) {
        let slot = &mut self.slots[kind.index()];
        if slot.restarts < MAX_RESTARTS {
            slot.restarts += 1;
            slot.restart_at = Some(Instant::now() + RESTART_DELAY * slot.restarts);
        } else {
            crate::pwarn!("bluetooth: {} watcher keeps failing; giving up", kind.label());
        }
    }

    /// Apply a possible connection change of device `key` (`was` = state
    /// before the endpoint update).
    fn transition(&mut self, key: &str, was: bool, at: Instant, silent: bool) {
        let Some(dev) = self.devices.get_mut(key) else { return };
        if dev.connected() != was {
            dev.changed(at, silent);
        }
    }

    /// Report changes that have settled, and restart watchers that are due.
    fn tick(&mut self, now: Instant) {
        let settled: Vec<(String, bool)> = self.devices.iter_mut().filter_map(|(k, d)| d.settled(now).map(|c| (k.clone(), c))).collect();
        for (key, connected) in settled {
            self.report(&key, connected);
        }
        for kind in Kind::ALL {
            let slot = &mut self.slots[kind.index()];
            if slot.restart_at.is_some_and(|t| t <= now) {
                slot.restart_at = None;
                if self.start_watch(kind) {
                    crate::pinfo!("bluetooth: {} watcher restarted", kind.label());
                } else {
                    self.schedule_restart(kind);
                }
            }
        }
    }

    /// Look up details (bounded) and send the pop-up event.
    fn report(&mut self, key: &str, connected: bool) {
        let Some(dev) = self.devices.get(key) else { return };
        let snap = Snapshot { container: dev.container, endpoints: dev.endpoints.clone(), audio: dev.audio };
        let (audio, battery) = describe(&snap, connected);

        let Some(dev) = self.devices.get_mut(key) else { return };
        if audio.is_some() {
            dev.audio = audio;
        }
        dev.shown = connected;
        let state = if connected { "connected" } else { "disconnected" };
        if audio == Some(false) {
            crate::pinfo!("bluetooth: a device that isn't headphones or a speaker {state} (no pop-up)");
            return;
        }
        let name = if dev.name.is_empty() { FALLBACK_NAME.to_string() } else { dev.name.clone() };
        let audio = audio.unwrap_or(false);
        crate::pinfo!("bluetooth: device {state} (audio {audio}, battery {})", battery.map_or_else(|| "unknown".to_string(), |b| format!("{b}%")));
        self.sink.send(AppEvent::Bluetooth { name, connected, battery, audio });
    }
}

// ---------------------------------------------------------------------------
// Lookups (worker thread only; every wait is bounded).

/// `(audio, battery)` for a report. Battery only for connects.
fn describe(dev: &Snapshot, connected: bool) -> (Option<bool>, Option<u8>) {
    let deadline = Instant::now() + LOOKUP_BUDGET;
    let mut audio = dev.audio.or_else(|| audio_from_hints(&dev.endpoints));
    let mut battery = None;

    if audio.is_none() {
        if let Some(ep) = dev.endpoint(Kind::Classic) {
            audio = classic_is_audio(&ep.id, soon(OPEN_TIMEOUT, deadline));
        }
    }
    // Not audio gear: no pop-up, so no battery to look up.
    if audio == Some(false) {
        return (audio, None);
    }
    if connected {
        if let Some(c) = &dev.container {
            battery = pnp_battery(c, soon(PNP_TIMEOUT, deadline));
        }
    }
    let want_battery = connected && battery.is_none();
    if want_battery || audio.is_none() {
        if let Some(ep) = dev.endpoint(Kind::Le) {
            let probe = le_probe(&ep.id, want_battery && ep.connected, audio.is_none(), soon(GATT_TIMEOUT, deadline));
            audio = audio.or(probe.audio);
            battery = battery.or(probe.battery);
        }
    }
    (audio, battery)
}

/// `Some(true)` if any endpoint's class says audio, `Some(false)` if every
/// endpoint's class is known and none does, else `None`.
fn audio_from_hints(endpoints: &[Endpoint]) -> Option<bool> {
    let mut all_known = !endpoints.is_empty();
    for e in endpoints {
        match e.class_hint {
            Some(h) if is_audio_class(e.kind, h) => return Some(true),
            Some(_) => {}
            None => all_known = false,
        }
    }
    all_known.then_some(false)
}

fn is_audio_class(kind: Kind, hint: u32) -> bool {
    match kind {
        // Major class number, or a raw Class of Device (major in bits 8..=12).
        Kind::Classic => {
            let major = if hint > 0x1F { (hint >> 8) & 0x1F } else { hint };
            major == BluetoothMajorClass::AudioVideo.0 as u32
        }
        Kind::Le => LE_AUDIO_CATEGORIES.contains(&hint),
    }
}

/// Classic device: Class of Device major class.
fn classic_is_audio(id: &str, deadline: Instant) -> Option<bool> {
    let dev = wait(deadline, || BluetoothDevice::FromIdAsync(&HSTRING::from(id)))?;
    let major = dev.ClassOfDevice().and_then(|c| c.MajorClass());
    let _ = dev.Close();
    major.ok().filter(|m| *m != BluetoothMajorClass::Miscellaneous).map(|m| m == BluetoothMajorClass::AudioVideo)
}

/// The Windows-maintained battery level on any PnP node of the device.
fn pnp_battery(container: &GUID, deadline: Instant) -> Option<u8> {
    let aqs = HSTRING::from(format!("System.Devices.ContainerId:=\"{}\"", braced(container)));
    let props = IIterable::<HSTRING>::try_from(vec![HSTRING::from(PKEY_BATTERY)]).ok()?;
    let nodes = wait(deadline, || DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&aqs, &props, DeviceInformationKind::Device))?;
    let n = nodes.Size().ok()?;
    (0..n).find_map(|i| {
        let p = nodes.GetAt(i).ok()?.Properties().ok()?;
        prop_u32(&p, PKEY_BATTERY).filter(|v| *v <= 100).map(|v| v as u8)
    })
}

#[derive(Default)]
struct LeProbe {
    audio: Option<bool>,
    battery: Option<u8>,
}

/// LE device: appearance category and/or the GATT battery level.
fn le_probe(id: &str, want_battery: bool, want_audio: bool, deadline: Instant) -> LeProbe {
    let mut out = LeProbe::default();
    let Some(dev) = wait(deadline, || BluetoothLEDevice::FromIdAsync(&HSTRING::from(id))) else { return out };
    if want_audio {
        out.audio = dev.Appearance().and_then(|a| a.Category()).ok().filter(|c| *c != 0).map(|c| is_audio_class(Kind::Le, u32::from(c)));
    }
    if want_battery && out.audio != Some(false) {
        out.battery = gatt_battery(&dev, deadline);
    }
    let _ = dev.Close();
    out
}

/// Battery Service 0x180F / Battery Level 0x2A19.
fn gatt_battery(dev: &BluetoothLEDevice, deadline: Instant) -> Option<u8> {
    let service_uuid = GattServiceUuids::Battery().ok()?;
    let level_uuid = GattCharacteristicUuids::BatteryLevel().ok()?;
    let mut found = None;
    for mode in [BluetoothCacheMode::Cached, BluetoothCacheMode::Uncached] {
        // After a timeout the next `wait` returns at once (deadline passed).
        let Some(result) = wait(deadline, || dev.GetGattServicesForUuidWithCacheModeAsync(service_uuid, mode)) else { continue };
        if result.Status().ok() != Some(GattCommunicationStatus::Success) {
            continue;
        }
        let Ok(services) = result.Services() else { continue };
        let n = services.Size().unwrap_or(0);
        for i in 0..n {
            let Ok(service) = services.GetAt(i) else { continue };
            if found.is_none() {
                found = read_battery_level(&service, level_uuid, deadline);
            }
            // Each service object holds a GATT session reference.
            let _ = service.Close();
        }
        if n > 0 {
            break;
        }
    }
    found
}

fn read_battery_level(service: &GattDeviceService, level_uuid: GUID, deadline: Instant) -> Option<u8> {
    let chars = wait(deadline, || service.GetCharacteristicsForUuidWithCacheModeAsync(level_uuid, BluetoothCacheMode::Cached))?;
    if chars.Status().ok()? != GattCommunicationStatus::Success {
        return None;
    }
    let list = chars.Characteristics().ok()?;
    if list.Size().ok()? == 0 {
        return None;
    }
    let characteristic = list.GetAt(0).ok()?;
    let read = wait(deadline, || characteristic.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached))?;
    if read.Status().ok()? != GattCommunicationStatus::Success {
        return None;
    }
    let value = read.Value().ok()?;
    if value.Length().ok()? == 0 {
        return None;
    }
    let level = DataReader::FromBuffer(&value).ok()?.ReadByte().ok()?;
    (level <= 100).then_some(level)
}

/// Run a WinRT async operation and wait for it until `deadline` (cancelling
/// it on timeout). Blocks the calling worker thread only.
fn wait<T, F>(deadline: Instant, start: F) -> Option<T>
where
    T: windows::core::RuntimeType + 'static,
    F: FnOnce() -> Result<IAsyncOperation<T>>,
{
    let left = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero())?;
    let op = start().ok()?;
    let (tx, rx) = mpsc::channel();
    let handler = AsyncOperationCompletedHandler::<T>::new(move |_, status| {
        let _ = tx.send(status);
        Ok(())
    });
    if op.SetCompleted(&handler).is_err() {
        let _ = op.Cancel();
        return None;
    }
    match rx.recv_timeout(left) {
        Ok(AsyncStatus::Completed) => op.GetResults().ok(),
        Ok(_) => None,
        Err(_) => {
            let _ = op.Cancel();
            None
        }
    }
}

/// `now + limit`, but never past `deadline`.
fn soon(limit: Duration, deadline: Instant) -> Instant {
    (Instant::now() + limit).min(deadline)
}

// ---------------------------------------------------------------------------
// Property helpers.

fn prop_value(map: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<IPropertyValue> {
    let key = HSTRING::from(key);
    if !map.HasKey(&key).unwrap_or(false) {
        return None;
    }
    // A present-but-empty property comes back as null, which is an Err here.
    map.Lookup(&key).ok()?.cast::<IPropertyValue>().ok()
}

fn prop_u32(map: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<u32> {
    let v = prop_value(map, key)?;
    match v.Type().ok()? {
        PropertyType::UInt8 => v.GetUInt8().ok().map(u32::from),
        PropertyType::UInt16 => v.GetUInt16().ok().map(u32::from),
        PropertyType::UInt32 => v.GetUInt32().ok(),
        PropertyType::Int16 => v.GetInt16().ok().and_then(|x| u32::try_from(x).ok()),
        PropertyType::Int32 => v.GetInt32().ok().and_then(|x| u32::try_from(x).ok()),
        _ => None,
    }
}

fn prop_bool(map: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<bool> {
    let v = prop_value(map, key)?;
    (v.Type().ok()? == PropertyType::Boolean).then(|| v.GetBoolean().ok()).flatten()
}

fn prop_guid(map: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<GUID> {
    let v = prop_value(map, key)?;
    (v.Type().ok()? == PropertyType::Guid).then(|| v.GetGuid().ok()).flatten()
}

/// Real device containers only (not "none" or the local machine's).
fn usable_container(g: &GUID) -> bool {
    const LOCAL_MACHINE: GUID = GUID::from_u128(0x00000000_0000_0000_ffff_ffffffffffff);
    *g != GUID::zeroed() && *g != LOCAL_MACHINE
}

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, the form AQS expects.
fn braced(g: &GUID) -> String {
    let d = g.data4;
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

/// `aqs` narrowed to paired devices. It goes in brackets, so an OR inside it
/// can't leave the new condition out.
fn paired_only(aqs: &str) -> String {
    format!("({aqs}) AND {AQS_PAIRED}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(kind: Kind, connected: bool, class_hint: Option<u32>) -> Endpoint {
        Endpoint { id: format!("{kind:?}"), kind, connected, class_hint }
    }

    fn headset() -> Device {
        Device { name: "Sound Buds".into(), container: None, endpoints: vec![endpoint(Kind::Classic, false, Some(4))], audio: None, shown: false, due: None }
    }

    /// What `tick` does at `now` with a settled change: report it (which
    /// marks it shown).
    fn report(dev: &mut Device, now: Instant) -> Option<bool> {
        let r = dev.settled(now);
        if let Some(c) = r {
            dev.shown = c;
        }
        r
    }

    /// The headset's endpoint (dis)connects at `at`, as the worker applies a
    /// watcher message, then the tick the worker runs after every message.
    fn set(dev: &mut Device, connected: bool, at: Instant) -> Option<bool> {
        let was = dev.connected();
        dev.endpoints[0].connected = connected;
        if dev.connected() != was {
            dev.changed(at, false);
        }
        report(dev, at)
    }

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn a_connection_pops_up_once_it_has_held() {
        let t0 = Instant::now();
        let mut dev = headset();
        assert_eq!(set(&mut dev, true, t0), None, "not at once");
        assert_eq!(report(&mut dev, t0 + SETTLE - secs(0.001)), None, "still settling");
        assert_eq!(report(&mut dev, t0 + SETTLE), Some(true));
        assert_eq!(report(&mut dev, t0 + SETTLE * 2), None, "reported once");
        assert_eq!(set(&mut dev, false, t0 + SETTLE * 3), None, "not at once");
        assert_eq!(report(&mut dev, t0 + SETTLE * 4), Some(false));
    }

    #[test]
    fn a_device_that_comes_and_goes_shows_nothing() {
        let t0 = Instant::now();
        let mut dev = headset();
        assert_eq!(set(&mut dev, true, t0), None);
        assert_eq!(set(&mut dev, false, t0 + secs(1.0)), None);
        for s in 0..40 {
            assert_eq!(report(&mut dev, t0 + secs(f64::from(s) * 0.25)), None);
        }
        // The same for a short drop of a device shown as connected.
        set(&mut dev, true, t0 + secs(10.0));
        assert_eq!(report(&mut dev, t0 + secs(13.0)), Some(true));
        assert_eq!(set(&mut dev, false, t0 + secs(20.0)), None);
        assert_eq!(set(&mut dev, true, t0 + secs(21.0)), None);
        assert_eq!(report(&mut dev, t0 + secs(30.0)), None);
    }

    #[test]
    fn every_change_starts_the_wait_again() {
        let t0 = Instant::now();
        let mut dev = headset();
        set(&mut dev, true, t0);
        set(&mut dev, false, t0 + secs(2.0));
        set(&mut dev, true, t0 + secs(4.0));
        assert_eq!(report(&mut dev, t0 + secs(6.0)), None);
        assert_eq!(report(&mut dev, t0 + secs(4.0) + SETTLE), Some(true));
    }

    #[test]
    fn devices_connected_at_start_are_not_news() {
        let t0 = Instant::now();
        let mut dev = headset();
        dev.endpoints[0].connected = true;
        dev.changed(t0, true);
        assert_eq!(report(&mut dev, t0 + SETTLE), None);
        set(&mut dev, false, t0 + SETTLE);
        assert_eq!(report(&mut dev, t0 + SETTLE * 2), Some(false));
    }

    #[test]
    fn only_audio_gear_says_it_is_audio() {
        let one = |kind, hint| audio_from_hints(&[endpoint(kind, true, hint)]);
        // Classic major classes: 4 = audio/video, 2 = phone, 5 = mouse, keyboard
        // or game controller; 0x24_0404 = a headset's whole Class of Device.
        assert_eq!(one(Kind::Classic, Some(4)), Some(true));
        assert_eq!(one(Kind::Classic, Some(0x24_0404)), Some(true));
        assert_eq!(one(Kind::Classic, Some(2)), Some(false));
        assert_eq!(one(Kind::Classic, Some(5)), Some(false));
        // LE appearance categories: 0x25 = earbuds, 0x03 = watch, 0x0F = HID.
        assert_eq!(one(Kind::Le, Some(0x25)), Some(true));
        assert_eq!(one(Kind::Le, Some(0x03)), Some(false));
        assert_eq!(one(Kind::Le, Some(0x0F)), Some(false));
        // A headset whose LE side says nothing is still a headset; a device
        // that says nothing at all may be one, so it still pops up.
        assert_eq!(audio_from_hints(&[endpoint(Kind::Le, true, None), endpoint(Kind::Classic, true, Some(4))]), Some(true));
        assert_eq!(one(Kind::Le, None), None);
    }

    #[test]
    fn the_filter_keeps_windows_own_whole() {
        assert_eq!(paired_only("A AND (B OR C)"), "(A AND (B OR C)) AND System.Devices.Aep.IsPaired:=System.StructuredQueryType.Boolean#True");
    }

    /// How a watcher on `aqs`, started as the worker starts it, ends its
    /// first listing (and how many devices it listed).
    fn first_listing(kind: Kind, aqs: &HSTRING) -> (String, usize) {
        let (tx, rx) = mpsc::channel();
        let watch = match Watch::start(kind, aqs, 0, 1, &tx) {
            Ok(w) => w,
            Err(e) => return (format!("didn't start ({e})"), 0),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut devices = 0;
        let outcome = loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Msg::Enumerated { .. }) => break "listed",
                Ok(Msg::Stopped { aborted: true, .. }) => break "aborted",
                Ok(Msg::Stopped { .. }) => break "stopped",
                Ok(Msg::Added { .. }) => devices += 1,
                Ok(_) => {}
                Err(_) => break "still listing after 10 s",
            }
        };
        watch.stop();
        (outcome.to_string(), devices)
    }

    /// Windows reads a watcher's filter only when the watcher starts, and a
    /// filter it refused would leave the island with no Bluetooth pop-ups at
    /// all. On a real Windows (CI's Unit tests job runs this) the paired-only
    /// filters must start and finish their first listing like Windows' own.
    #[test]
    #[ignore = "needs a real Windows; CI runs it on its own"]
    fn the_paired_only_filters_work_like_windows_own() {
        let com = ComApartment::enter_mta();
        assert!(com.initialized);
        for kind in Kind::ALL {
            let own = kind.connected_selector().expect("Windows' own filter");
            let ours = kind.selector().expect("the paired-only filter");
            println!("{} filter from Windows: {own}", kind.label());
            println!("{} filter used:         {ours}", kind.label());
            let ((a, n), (b, m)) = (first_listing(kind, &own), first_listing(kind, &ours));
            println!("{}: Windows' own {a} ({n} connected), paired-only {b} ({m} connected)", kind.label());
            assert_eq!(b, a, "the {} watcher's first listing", kind.label());
            assert!(m <= n, "the paired-only {} filter listed more devices than Windows' own", kind.label());
        }
    }
}
