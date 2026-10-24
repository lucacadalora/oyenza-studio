//! Platform-independent models and logic for Poni.
//!
//! Everything here is pure Rust with no OS calls so it can be unit-tested on
//! any machine. The Windows crate (`poni`) owns all I/O and rendering and
//! feeds clocks/dates into these types. Two modules use `std` I/O, which
//! still runs anywhere: `store` (the JSON files) and `phonedrop` (its small
//! web server, so the tests can drive it over real sockets).

pub mod applemusic;
pub mod cliphist;
pub mod datetime;
pub mod frametime;
pub mod geom;
pub mod glass;
pub mod health;
pub mod look;
pub mod lrc;
pub mod lrclib;
pub mod nowplaying;
pub mod oyen;
pub mod oyenza;
pub mod phonedrop;
pub mod pomodoro;
pub mod romanize;
pub mod settings;
pub mod shelf;
pub mod spring;
pub mod store;
pub mod svgpath;
pub mod sysmon;
pub mod timeline;
pub mod todo;
pub mod ttml;
pub mod urlenc;
