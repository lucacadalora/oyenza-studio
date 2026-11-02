//! Where the island sits along the top of its screen: the middle, or the
//! spot it was dragged to (`Settings::island_x`). Only sideways: it always
//! hangs from (or floats just below) the top edge, and opens downwards.
//!
//! The window is sized for the open island and never leaves the monitor.
//! Near an edge the window stops and the island goes the rest of the way
//! inside it, kept in by its own width: a closed island reaches the edge,
//! and opening there it grows away from the edge instead of off the screen.

/// Pointer travel (DIPs, sideways) before a press on the closed island
/// becomes a drag. Less is a click, which opens the island as before.
pub const DRAG_SLOP: f32 = 6.0;

/// Dropped (or dragged) this close to the middle (DIPs), the island goes
/// back to the centre, where it was before it ever moved.
pub const SNAP: f32 = 16.0;

/// The window's left edge (px) for an island centred `at` px from the
/// screen's left: a `win_w` px window on it, kept on the `mon_w` px wide
/// monitor that starts at `mon_left`. A window wider than the monitor
/// stays centred on it.
pub fn window_left(mon_left: i32, mon_w: i32, win_w: i32, at: i32) -> i32 {
    if win_w >= mon_w {
        return mon_left + (mon_w - win_w) / 2;
    }
    (at - win_w / 2).clamp(mon_left, mon_left + mon_w - win_w)
}

/// The island's centre (DIPs) along something `room` wide, as near `want`
/// as an outline `body_w` wide with `side` to spare on both sides allows.
/// With no room for it, the middle.
pub fn fit_centre(want: f32, room: f32, body_w: f32, side: f32) -> f32 {
    let lo = body_w / 2.0 + side;
    let hi = room - body_w / 2.0 - side;
    if lo > hi {
        room / 2.0
    } else {
        want.clamp(lo, hi)
    }
}

/// What to remember for an island centred `centre` DIPs across a monitor
/// `mon_w` DIPs wide: its share of the width (the same spot on any screen
/// and scale), or nothing near the middle.
pub fn remember(centre: f32, mon_w: f32) -> Option<f32> {
    if mon_w.is_nan() || mon_w <= 0.0 || !centre.is_finite() || (centre - mon_w / 2.0).abs() < SNAP {
        return None;
    }
    Some((centre / mon_w).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_follows_the_island_until_the_monitors_edge() {
        // A 700 px window on a 1920 px monitor that starts at 0.
        assert_eq!(window_left(0, 1920, 700, 960), 610);
        assert_eq!(window_left(0, 1920, 700, 1500), 1150);
        // Past the edge it stops, whole, on the monitor.
        assert_eq!(window_left(0, 1920, 700, 1900), 1220);
        assert_eq!(window_left(0, 1920, 700, 10), 0);
        // A second monitor to the left of the main one.
        assert_eq!(window_left(-1280, 1280, 700, -20), -700);
        assert_eq!(window_left(-1280, 1280, 700, -1270), -1280);
        // Wider than the monitor: centred, as the window always was.
        assert_eq!(window_left(0, 640, 700, 600), -30);
    }

    #[test]
    fn the_window_on_the_middle_is_where_it_always_was() {
        // `surface_size` makes monitor and window widths the same parity,
        // so the halves give the old `left + (mon_w - w) / 2`.
        for (mon_w, w) in [(1920, 700), (1365, 691), (2560, 1036), (1366, 692)] {
            assert_eq!(window_left(100, mon_w, w, 100 + mon_w / 2), 100 + (mon_w - w) / 2, "{mon_w} {w}");
        }
    }

    #[test]
    fn the_island_goes_the_rest_of_the_way_inside_the_window() {
        // Window 700 DIPs, island 200 wide with 20 to spare each side.
        assert_eq!(fit_centre(350.0, 700.0, 200.0, 20.0), 350.0);
        assert_eq!(fit_centre(650.0, 700.0, 200.0, 20.0), 580.0);
        assert_eq!(fit_centre(0.0, 700.0, 200.0, 20.0), 120.0);
        // Opening there, it grows away from the edge: 660 wide fills the window.
        assert_eq!(fit_centre(650.0, 700.0, 660.0, 20.0), 350.0);
        assert_eq!(fit_centre(650.0, 700.0, 500.0, 20.0), 430.0);
        // An outline wider than the window (it outgrew it for a frame): the middle, never a panic.
        assert_eq!(fit_centre(650.0, 700.0, 800.0, 20.0), 350.0);
    }

    #[test]
    fn near_the_middle_is_the_centre() {
        assert_eq!(remember(768.0, 1536.0), None);
        assert_eq!(remember(768.0 + SNAP - 0.5, 1536.0), None);
        assert_eq!(remember(1152.0, 1536.0), Some(0.75));
        assert_eq!(remember(-40.0, 1536.0), Some(0.0));
        assert_eq!(remember(f32::NAN, 1536.0), None);
        assert_eq!(remember(100.0, 0.0), None);
    }
}
