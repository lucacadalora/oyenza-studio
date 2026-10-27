//! Prayer times' cities outside Indonesia, looked up online one small file
//! at a time (`poni_core::prayer::world`: the two letters typed in the place
//! picker, or a 5-degree square around a location). Shared by the Windows
//! and Mac apps.
//!
//! Each file gets its own short-lived thread: it asks once, sends
//! [`AppEvent::Cities`] and ends. Nothing runs while no file is wanted.

use crate::events::{AppEvent, EventSink};
use crate::services::http::{Client, MtaGuard};
use poni_core::prayer::{world, World};
use std::time::Duration;

const USER_AGENT: &str = concat!("OyenzaStudio/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lucacadalora/oyenza-studio)");

/// The biggest file is under 100 KB; this is plenty on a slow line.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Get `file` ("ed", "near/55_-5"); the answer comes as `AppEvent::Cities`.
pub fn fetch(file: String, sink: EventSink) {
    let spawned = std::thread::Builder::new().name("poni-cities".into()).spawn({
        let file = file.clone();
        let sink = sink.clone();
        move || {
            let got = get(&file);
            if let Err(e) = &got {
                crate::pwarn!("prayer: cities {file}: {e}");
            }
            sink.send(AppEvent::Cities { file, got });
        }
    });
    if let Err(e) = spawned {
        sink.send(AppEvent::Cities { file, got: Err(format!("no thread ({e})")) });
    }
}

fn get(file: &str) -> Result<World, String> {
    let _com = MtaGuard::new();
    let client = Client::new(USER_AGENT).map_err(|e| e.to_string())?;
    let resp = client.get(&world::url(file), TIMEOUT).map_err(|e| e.to_string())?;
    match resp.status {
        200 => World::parse(resp.body),
        // No file: no city has a name starting so, or none is in that square.
        404 => Ok(World::empty()),
        s => Err(format!("HTTP {s}")),
    }
}
