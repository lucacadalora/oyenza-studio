//! Platform-independent models and logic for Poni.
//!
//! Everything here is pure Rust with no OS calls so it can be unit-tested on
//! any machine. The Windows crate (`poni`) owns all I/O and rendering and
//! feeds clocks/dates into these types. Three modules use `std` I/O, which
//! still runs anywhere: `store` (the JSON files), `phonedrop` (its small
//! web server) and `pairdrop` (UDP sockets for the direct link to a paired
//! phone), so the tests can drive them over real sockets.

pub mod addons;
pub mod agenda;
pub mod applemusic;
pub mod datetime;
pub mod downloads;
pub mod frames;
pub mod frametime;
pub mod geom;
pub mod glass;
pub mod hdr;
pub mod health;
pub mod ielts;
pub mod look;
pub mod lrc;
pub mod lrclib;
pub mod mediaremote;
pub mod notes;
pub mod notifications;
pub mod nowplaying;
pub mod oyen;
pub mod oyenza;
pub mod pairdrop;
pub mod peek;
pub mod phonedrop;
pub mod pomodoro;
pub mod power;
pub mod prayer;
pub mod recorder;
pub mod romanize;
pub mod script;
pub mod settings;
pub mod sha256;
pub mod shelf;
pub mod spring;
pub mod stocks;
pub mod store;
pub mod svgpath;
pub mod sysmon;
pub mod timeline;
pub mod todo;
pub mod ttml;
pub mod update;
pub mod urlenc;
pub mod ytdlp;
pub mod ytdlp_info;
pub mod ytdlp_partial;
