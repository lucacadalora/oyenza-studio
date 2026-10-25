//! Where the Mac is, for the Prayer times add-on: macOS's Location Services
//! (Core Location), asked on the main thread. macOS decides how (Wi-Fi,
//! mostly) and whether Oyenza Studio may ask: the first time it asks the
//! person itself, and System Settings › Privacy & Security › Location
//! Services changes the answer later.
//!
//! The point only picks the nearest place in the add-on's list (the app
//! does that as soon as it arrives); it is never stored or sent anywhere.
//! Nothing runs between asks: the app lets go of the manager once the
//! answer is in ([`forget`]), and an ask that gets none ends after
//! [`TIMEOUT`].

use crate::events::{AppEvent, EventSink};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_core_location::{CLAuthorizationStatus, CLError, CLLocation, CLLocationManager, CLLocationManagerDelegate};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use std::cell::{Cell, RefCell};
use std::time::Duration;

/// The longest one ask may take, macOS's question to the person included.
const TIMEOUT: Duration = Duration::from_secs(60);
/// A kab/kota is all the add-on needs, and a coarse fix comes fastest (metres).
const ACCURACY_M: f64 = 3000.0;

/// What asking macOS for the Mac's location came to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Located {
    /// Latitude and longitude, degrees.
    At(f64, f64),
    /// Location Services is off, or not allowed for Oyenza Studio.
    Denied,
    /// macOS couldn't tell where the Mac is (no Wi-Fi around, no answer).
    Unknown,
}

/// The ask in progress (or answered, until the app lets go of it).
struct Ask {
    manager: Retained<CLLocationManager>,
    _delegate: Retained<LocationDelegate>,
    sink: EventSink,
    /// Which ask this is: a timeout left from an earlier one does nothing.
    n: u64,
    /// macOS's question is up (asked once per ask).
    asked_person: bool,
    /// The fix is asked for (once per ask).
    requested: bool,
    answered: bool,
}

thread_local! {
    static ASK: RefCell<Option<Ask>> = const { RefCell::new(None) };
    static ASKS: Cell<u64> = const { Cell::new(0) };
}

/// Ask once; the answer comes back as `AppEvent::Located`.
pub fn locate(mtm: MainThreadMarker, sink: EventSink) {
    forget();
    let manager = unsafe { CLLocationManager::new() };
    let delegate: Retained<LocationDelegate> = unsafe { msg_send![LocationDelegate::alloc(mtm), init] };
    unsafe {
        manager.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        manager.setDesiredAccuracy(ACCURACY_M);
    }
    let n = ASKS.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    ASK.with(|a| *a.borrow_mut() = Some(Ask { manager, _delegate: delegate, sink, n, asked_person: false, requested: false, answered: false }));
    // macOS also says where the permission stands right after the manager
    // starts (`locationManagerDidChangeAuthorization:`); `step` runs once
    // for each change and does each thing only once.
    step();
    let gave_up = dispatch2::DispatchTime::try_from(TIMEOUT).map(|t| {
        dispatch2::DispatchQueue::main().after(t, move || {
            if ASK.with(|a| a.borrow().as_ref().is_some_and(|a| a.n == n && !a.answered)) {
                answer(Located::Unknown);
            }
        })
    });
    if !matches!(gave_up, Ok(Ok(()))) {
        crate::pwarn!("location: couldn't set its time limit");
    }
}

/// Let go of the manager (the app has the answer, or Prayer times went off).
pub fn forget() {
    let ask = ASK.with(|a| a.borrow_mut().take());
    if let Some(a) = ask {
        unsafe {
            a.manager.setDelegate(None);
            a.manager.stopUpdatingLocation();
        }
    }
}

/// Ask macOS for what the permission allows: its question while nobody has
/// answered it, the fix once allowed.
fn step() {
    let Some((manager, asked_person, requested)) = ASK.with(|a| a.borrow().as_ref().filter(|a| !a.answered).map(|a| (a.manager.clone(), a.asked_person, a.requested))) else {
        return;
    };
    // Outside the borrow: Core Location may call the delegate before returning.
    match unsafe { manager.authorizationStatus() } {
        CLAuthorizationStatus::NotDetermined => {
            if !asked_person {
                set(|a| a.asked_person = true);
                unsafe { manager.requestWhenInUseAuthorization() };
            }
        }
        CLAuthorizationStatus::Denied | CLAuthorizationStatus::Restricted => answer(Located::Denied),
        _ => {
            if !requested {
                set(|a| a.requested = true);
                unsafe { manager.requestLocation() };
            }
        }
    }
}

fn set(f: impl FnOnce(&mut Ask)) {
    ASK.with(|a| {
        if let Some(a) = a.borrow_mut().as_mut() {
            f(a);
        }
    });
}

/// Send the answer once; the manager stays until the app calls [`forget`]
/// (letting go of it inside its own callback would free it mid-call).
fn answer(found: Located) {
    let sink = ASK.with(|a| {
        let mut a = a.borrow_mut();
        let a = a.as_mut().filter(|a| !a.answered)?;
        a.answered = true;
        Some(a.sink.clone())
    });
    let Some(sink) = sink else { return };
    crate::pinfo!(
        "location: {}",
        match found {
            Located::At(..) => "found",
            Located::Denied => "not allowed",
            Located::Unknown => "unknown",
        }
    );
    sink.send(AppEvent::Located(found));
}

define_class!(
    /// Hears Core Location's answers (main thread).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PoniLocationDelegate"]
    struct LocationDelegate;

    unsafe impl NSObjectProtocol for LocationDelegate {}

    unsafe impl CLLocationManagerDelegate for LocationDelegate {
        #[unsafe(method(locationManagerDidChangeAuthorization:))]
        fn did_change_authorization(&self, _manager: &CLLocationManager) {
            step();
        }

        #[unsafe(method(locationManager:didUpdateLocations:))]
        fn did_update_locations(&self, _manager: &CLLocationManager, locations: &NSArray<CLLocation>) {
            let Some(last) = locations.lastObject() else { return };
            let c = unsafe { last.coordinate() };
            answer(if c.latitude.is_finite() && c.longitude.is_finite() { Located::At(c.latitude, c.longitude) } else { Located::Unknown });
        }

        #[unsafe(method(locationManager:didFailWithError:))]
        fn did_fail(&self, _manager: &CLLocationManager, error: &NSError) {
            answer(if error.code() == CLError::Denied.0 { Located::Denied } else { Located::Unknown });
        }
    }
);
