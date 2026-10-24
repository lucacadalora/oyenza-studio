//! Drag and drop state for the Shelf. The view (`window.rs`) is the drop
//! target and the drag source; this holds whether a drag out of the island
//! is running (the island refuses its own drags, and the Shelf page re-checks
//! the dragged file afterwards).

use std::sync::atomic::{AtomicBool, Ordering};

static DRAGGING_OUT: AtomicBool = AtomicBool::new(false);

pub fn is_dragging_out() -> bool {
    DRAGGING_OUT.load(Ordering::Relaxed)
}

pub fn set_dragging_out(on: bool) {
    DRAGGING_OUT.store(on, Ordering::Relaxed);
}
