//! Vector icons drawn on a 24×24 grid as SVG path data (original artwork).
//!
//! Stroked parts use a 2-unit round-capped stroke; filled parts are solid.
//! Geometry is parsed once and cached by [`crate::gfx::Gfx`].

pub const STROKE: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Play,
    Pause,
    Next,
    Prev,
    Music,
    Lyrics,
    Timer,
    Coffee,
    Steps,
    Tray,
    ListChecks,
    Check,
    CheckCircle,
    Circle,
    Plus,
    Minus,
    Close,
    Clipboard,
    Copy,
    Calendar,
    Clock,
    Cpu,
    Memory,
    Disk,
    Battery,
    Bolt,
    Bluetooth,
    Headphones,
    Volume,
    VolumeLow,
    VolumeMute,
    Pin,
    PinFill,
    Folder,
    File,
    Trash,
    Undo,
    Reset,
    Sliders,
    Open,
    Phone,
    /// Oyenza: show a hidden sum.
    Eye,
    /// Oyenza: money that arrived.
    MoneyIn,
    /// Oyenza: money that went out.
    MoneyOut,
    /// Oyenza: money moved between the person's own wallets.
    Transfer,
    /// Oyenza: a credit card bill.
    Card,
    /// Oyenza: the tab.
    Wallet,
    /// Oyenza: signed out.
    Unlink,
    /// The header's Settings button.
    Gear,
    /// The System tab: a dial with its needle.
    Gauge,
    /// An app whose own icon isn't known.
    App,
    /// The Windows mark, for Windows' own processes in the System page's lists.
    Windows,
    /// The Add-ons tab: three tiles and a plus where the fourth would be.
    Addons,
    /// The Script tab: lines of text with a cue arrow at the line to say.
    Script,
    /// A crescent moon: Prayer times.
    Moon,
    /// Stop (the adzan): a rounded square.
    Stop,
    /// A new version: an arrow down into a tray.
    Download,
    /// Record (Oyenza Studio Recorder): a dot that fits inside `Circle`.
    Record,
    /// The Stocks tab: a line climbing to the right, its arrowhead at the end.
    Stocks,
    /// A magnifying glass: the Stocks page's search.
    Search,
    /// The Record tab: a ring with the record dot inside.
    Recorder,
    /// The Record page's targets: a display, an app window, a dragged-out area.
    Screen,
    AppWindow,
    Area,
    /// The Record page's inputs (system sound is `Volume`).
    Camera,
    Mic,
}

pub struct Part {
    pub d: &'static str,
    pub fill: bool,
}

const fn s(d: &'static str) -> Part {
    Part { d, fill: false }
}

const fn f(d: &'static str) -> Part {
    Part { d, fill: true }
}

const CIRCLE_9: &str = "M3 12a9 9 0 1 0 18 0a9 9 0 1 0-18 0";
const SPEAKER: &str = "M4 9.5h3l4.5-4v13L7 14.5H4z";

/// Builds a `&'static [Part]` (const-promoted so it can be returned).
macro_rules! p {
    ($($part:expr),* $(,)?) => {{
        const PARTS: &[Part] = &[$($part),*];
        PARTS
    }};
}

pub fn parts(icon: Icon) -> &'static [Part] {
    use Icon::*;
    match icon {
        Play => p![f("M8 5.6v12.8a1.1 1.1 0 0 0 1.7.93l10-6.4a1.1 1.1 0 0 0 0-1.86l-10-6.4A1.1 1.1 0 0 0 8 5.6z")],
        Pause => p![
            f("M7.2 5h1.6a1.2 1.2 0 0 1 1.2 1.2v11.6a1.2 1.2 0 0 1-1.2 1.2H7.2A1.2 1.2 0 0 1 6 17.8V6.2A1.2 1.2 0 0 1 7.2 5z"),
            f("M15.2 5h1.6a1.2 1.2 0 0 1 1.2 1.2v11.6a1.2 1.2 0 0 1-1.2 1.2h-1.6a1.2 1.2 0 0 1-1.2-1.2V6.2A1.2 1.2 0 0 1 15.2 5z"),
        ],
        Next => p![
            f("M4.5 6.3v11.4a.9.9 0 0 0 1.4.74l8.3-5.7a.9.9 0 0 0 0-1.48L5.9 5.56a.9.9 0 0 0-1.4.74z"),
            f("M17 5.5h1.6a.9.9 0 0 1 .9.9v11.2a.9.9 0 0 1-.9.9H17a.9.9 0 0 1-.9-.9V6.4a.9.9 0 0 1 .9-.9z"),
        ],
        Prev => p![
            f("M19.5 6.3v11.4a.9.9 0 0 1-1.4.74l-8.3-5.7a.9.9 0 0 1 0-1.48l8.3-5.7a.9.9 0 0 1 1.4.74z"),
            f("M5.4 5.5H7a.9.9 0 0 1 .9.9v11.2a.9.9 0 0 1-.9.9H5.4a.9.9 0 0 1-.9-.9V6.4a.9.9 0 0 1 .9-.9z"),
        ],
        Music => p![s("M9 18V5.5l11-2V16"), s("M3 18a3 3 0 1 0 6 0a3 3 0 1 0-6 0"), s("M14 16a3 3 0 1 0 6 0a3 3 0 1 0-6 0")],
        Lyrics => p![
            s("M5.5 4h13A1.5 1.5 0 0 1 20 5.5v10a1.5 1.5 0 0 1-1.5 1.5H10l-4 3.5V17h-.5A1.5 1.5 0 0 1 4 15.5v-10A1.5 1.5 0 0 1 5.5 4z"),
            s("M8 8.5h8"),
            s("M8 12h5"),
        ],
        Timer => p![s("M4 13a8 8 0 1 0 16 0a8 8 0 1 0-16 0"), s("M12 9v4l2.5 2.5"), s("M10 2.5h4"), s("M19 5.5l1.5 1.5")],
        Coffee => p![
            s("M5 9h11v5a5 5 0 0 1-5 5h-1a5 5 0 0 1-5-5z"),
            s("M16 10.5h1.5a2.5 2.5 0 0 1 0 5H16"),
            s("M8.5 3.5V6"),
            s("M12 3.5V6"),
        ],
        // Two shoe prints. Each outline starts at a corner of its flat bottom:
        // closing in the middle of a curve leaves a tiny segment whose join spikes.
        Steps => p![
            s("M5.2 12.5C4.8 11.1 4 9.7 4 7.6C4 4.8 5.5 2.5 7.5 2.5C9.5 2.5 11 4.8 11 7.6C11 9.7 10.2 11.1 9.8 12.5Z"),
            s("M5.5 15H9.5V15.6A2 2 0 0 1 5.5 15.6Z"),
            s("M14.2 17C13.8 15.6 13 14.2 13 12.1C13 9.3 14.5 7 16.5 7C18.5 7 20 9.3 20 12.1C20 14.2 19.2 15.6 18.8 17Z"),
            s("M14.5 19.5H18.5V20.1A2 2 0 0 1 14.5 20.1Z"),
        ],
        Tray => p![
            s("M4 13l2.4-7.2a1.6 1.6 0 0 1 1.5-1.1h8.2a1.6 1.6 0 0 1 1.5 1.1L20 13"),
            s("M4 13v5a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-5"),
            s("M4 13h4.5l1.5 2.5h4l1.5-2.5H20"),
        ],
        ListChecks => p![
            s("M11 6h9"),
            s("M11 12h9"),
            s("M11 18h9"),
            s("M3.5 6l1.6 1.6L8 4.7"),
            s("M3.5 12l1.6 1.6L8 10.7"),
            s("M4 18h3"),
        ],
        Check => p![s("M5 12.5l4.5 4.5L19 7.5")],
        CheckCircle => p![s(CIRCLE_9), s("M8 12.3l2.8 2.8L16.3 9.5")],
        Circle => p![s(CIRCLE_9)],
        Plus => p![s("M12 5v14"), s("M5 12h14")],
        Minus => p![s("M5 12h14")],
        Close => p![s("M6.5 6.5l11 11"), s("M17.5 6.5l-11 11")],
        Clipboard => p![
            s("M9 3.5h6a1 1 0 0 1 1 1V6a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1z"),
            s("M16 5h1.5A1.5 1.5 0 0 1 19 6.5v13a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 5 19.5v-13A1.5 1.5 0 0 1 6.5 5H8"),
        ],
        Copy => p![
            s("M9.5 8.5h9A1.5 1.5 0 0 1 20 10v9.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 8 19.5V10a1.5 1.5 0 0 1 1.5-1.5z"),
            s("M16 8.5v-3A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5V15a1.5 1.5 0 0 0 1.5 1.5H8"),
        ],
        Calendar => p![
            s("M5.5 5h13A1.5 1.5 0 0 1 20 6.5v12a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18.5v-12A1.5 1.5 0 0 1 5.5 5z"),
            s("M4 10h16"),
            s("M8.5 3v4"),
            s("M15.5 3v4"),
        ],
        Clock => p![s(CIRCLE_9), s("M12 7.5V12l3 2")],
        Cpu => p![
            s("M7.5 6h9A1.5 1.5 0 0 1 18 7.5v9a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 6 16.5v-9A1.5 1.5 0 0 1 7.5 6z"),
            s("M10 10h4v4h-4z"),
            s("M9.5 3v3M14.5 3v3M9.5 18v3M14.5 18v3M3 9.5h3M3 14.5h3M18 9.5h3M18 14.5h3"),
        ],
        Memory => p![
            s("M4 7.5h16a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1z"),
            s("M7 11v2M10.5 11v2M14 11v2M17.5 11v2"),
            s("M6 16.5v2M18 16.5v2"),
        ],
        Disk => p![
            s("M4 14l2.5-8A1.5 1.5 0 0 1 8 5h8a1.5 1.5 0 0 1 1.5 1l2.5 8"),
            s("M4 14h16v4a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18z"),
            s("M16.5 16.8h.01"),
        ],
        Battery => p![s("M4.5 7h12A1.5 1.5 0 0 1 18 8.5v7a1.5 1.5 0 0 1-1.5 1.5h-12A1.5 1.5 0 0 1 3 15.5v-7A1.5 1.5 0 0 1 4.5 7z"), s("M21 10.5v3")],
        Bolt => p![f("M13.4 2.5L5 13.6h6.2l-1.3 7.9L19 10.4h-6.3z")],
        Bluetooth => p![s("M7 7.5l10 9L12 21V3l5 4.5-10 9")],
        Headphones => p![
            s("M4 18v-5a8 8 0 0 1 16 0v5"),
            s("M4 15.5A1.5 1.5 0 0 1 5.5 14H7a1 1 0 0 1 1 1v4.5a1 1 0 0 1-1 1H5.5A1.5 1.5 0 0 1 4 19z"),
            s("M20 15.5a1.5 1.5 0 0 0-1.5-1.5H17a1 1 0 0 0-1 1v4.5a1 1 0 0 0 1 1h1.5A1.5 1.5 0 0 0 20 19z"),
        ],
        Volume => p![s(SPEAKER), s("M15 9a4 4 0 0 1 0 6"), s("M17.8 6.5a7.5 7.5 0 0 1 0 11")],
        VolumeLow => p![s(SPEAKER), s("M15 9a4 4 0 0 1 0 6")],
        VolumeMute => p![s(SPEAKER), s("M16 9.5l5 5"), s("M21 9.5l-5 5")],
        Pin => p![s("M9 4h6l-1 5.5 3 3V14H7v-1.5l3-3z"), s("M12 14v6")],
        PinFill => p![f("M9 4h6l-1 5.5 3 3V14H7v-1.5l3-3z"), s("M12 14v6")],
        Folder => p![s("M3.5 7.5A1.5 1.5 0 0 1 5 6h4l2 2h8a1.5 1.5 0 0 1 1.5 1.5v8A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5z")],
        File => p![s("M7.5 3H14l5 5v11.5a1.5 1.5 0 0 1-1.5 1.5h-10A1.5 1.5 0 0 1 6 19.5v-15A1.5 1.5 0 0 1 7.5 3z"), s("M14 3v5h5")],
        Trash => p![
            s("M4 7h16"),
            s("M9.5 7V5a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1v2"),
            s("M6.5 7l.9 12a1.5 1.5 0 0 0 1.5 1.4h6.2a1.5 1.5 0 0 0 1.5-1.4L17.5 7"),
        ],
        Undo => p![s("M9 14L4.5 9.5 9 5"), s("M4.5 9.5H14a5.5 5.5 0 0 1 0 11h-3")],
        Reset => p![s("M4 12a8 8 0 1 0 2.4-5.7"), s("M4 4v4.5h4.5")],
        Sliders => p![
            s("M4 6h10M18 6h2M16 4v4"),
            s("M4 12h4M12 12h8M10 10v4"),
            s("M4 18h12M20 18h.01M18 16v4"),
        ],
        Open => p![s("M14 4h6v6"), s("M20 4l-9 9"), s("M18 14v4.5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 4 18.5v-11A1.5 1.5 0 0 1 5.5 6H10")],
        Phone => p![s("M9 2.5h6A2.5 2.5 0 0 1 17.5 5v14a2.5 2.5 0 0 1-2.5 2.5H9A2.5 2.5 0 0 1 6.5 19V5A2.5 2.5 0 0 1 9 2.5z"), s("M11 18.5h2")],
        Eye => p![s("M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12z"), s("M9.2 12a2.8 2.8 0 1 0 5.6 0a2.8 2.8 0 1 0-5.6 0")],
        MoneyIn => p![s("M12 4v10.5"), s("M7.5 10.5L12 15l4.5-4.5"), s("M5 19.5h14")],
        MoneyOut => p![s("M12 20V9.5"), s("M7.5 13.5L12 9l4.5 4.5"), s("M5 4.5h14")],
        Transfer => p![s("M5 9h12.5"), s("M14.5 6l3 3-3 3"), s("M19 15H6.5"), s("M9.5 12l-3 3 3 3")],
        Card => p![
            s("M4.5 6.5h15A1.5 1.5 0 0 1 21 8v8a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 16V8a1.5 1.5 0 0 1 1.5-1.5z"),
            s("M3 10h18"),
            s("M6.5 14h4"),
        ],
        Wallet => p![
            s("M4 7.5h14.5A1.5 1.5 0 0 1 20 9v9a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18z"),
            s("M4 7.5l10.5-3v3"),
            s("M15.5 13.5h2"),
        ],
        Unlink => p![
            s("M9.5 14.5l-1.7 1.7a3 3 0 0 1-4.2-4.2l2.2-2.2a3 3 0 0 1 4.2 0"),
            s("M14.5 9.5l1.7-1.7a3 3 0 0 1 4.2 4.2l-2.2 2.2a3 3 0 0 1-4.2 0"),
            s("M4 4l16 16"),
        ],
        Gauge => p![s("M4.6 18.5A8.5 8.5 0 1 1 19.4 18.5"), s("M12 14.5l3.8-4.3"), f("M10.6 14.5a1.4 1.4 0 1 0 2.8 0a1.4 1.4 0 1 0-2.8 0")],
        App => p![
            s("M6.5 4h11A2.5 2.5 0 0 1 20 6.5v11a2.5 2.5 0 0 1-2.5 2.5h-11A2.5 2.5 0 0 1 4 17.5v-11A2.5 2.5 0 0 1 6.5 4z"),
            s("M4 8.5h16"),
        ],
        Windows => p![f("M4 4h7.5v7.5H4z"), f("M12.5 4H20v7.5h-7.5z"), f("M4 12.5h7.5V20H4z"), f("M12.5 12.5H20V20h-7.5z")],
        // Eight teeth (tops on r 9.5, valleys on r 7) around a hole. The outline
        // starts at a tooth's corner, not mid-curve, so its closing join can't spike.
        Gear => p![
            s("M10.25 5.22L10.43 2.63A9.5 9.5 0 0 1 13.57 2.63L13.75 5.22A7 7 0 0 1 15.55 5.97L17.52 4.27A9.5 9.5 0 0 1 19.73 6.48L18.03 8.45A7 7 0 0 1 18.78 10.25L21.37 10.43A9.5 9.5 0 0 1 21.37 13.57L18.78 13.75A7 7 0 0 1 18.03 15.55L19.73 17.52A9.5 9.5 0 0 1 17.52 19.73L15.55 18.03A7 7 0 0 1 13.75 18.78L13.57 21.37A9.5 9.5 0 0 1 10.43 21.37L10.25 18.78A7 7 0 0 1 8.45 18.03L6.48 19.73A9.5 9.5 0 0 1 4.27 17.52L5.97 15.55A7 7 0 0 1 5.22 13.75L2.63 13.57A9.5 9.5 0 0 1 2.63 10.43L5.22 10.25A7 7 0 0 1 5.97 8.45L4.27 6.48A9.5 9.5 0 0 1 6.48 4.27L8.45 5.97A7 7 0 0 1 10.25 5.22Z"),
            s("M9 12a3 3 0 1 0 6 0a3 3 0 1 0-6 0"),
        ],
        Addons => p![
            s("M5.5 4h3A1.5 1.5 0 0 1 10 5.5v3A1.5 1.5 0 0 1 8.5 10h-3A1.5 1.5 0 0 1 4 8.5v-3A1.5 1.5 0 0 1 5.5 4z"),
            s("M15.5 4h3A1.5 1.5 0 0 1 20 5.5v3a1.5 1.5 0 0 1-1.5 1.5h-3A1.5 1.5 0 0 1 14 8.5v-3A1.5 1.5 0 0 1 15.5 4z"),
            s("M5.5 14h3a1.5 1.5 0 0 1 1.5 1.5v3A1.5 1.5 0 0 1 8.5 20h-3A1.5 1.5 0 0 1 4 18.5v-3A1.5 1.5 0 0 1 5.5 14z"),
            s("M17 13.5v7"),
            s("M13.5 17h7"),
        ],
        // A circle (r 8) less a second one (r 7) up and to the right: both arcs meet where the circles cross.
        Moon => p![s("M19.77 13.9A8 8 0 1 1 10.1 4.23A7 7 0 0 0 19.77 13.9Z")],
        Script => p![s("M10 6.5h10"), s("M10 12h10"), s("M10 17.5h6.5"), f("M3.6 9.3v5.4a.8.8 0 0 0 1.2.7l4.3-2.7a.8.8 0 0 0 0-1.4L4.8 8.6a.8.8 0 0 0-1.2.7z")],
        Stop => p![f("M8.2 6.5h7.6a1.7 1.7 0 0 1 1.7 1.7v7.6a1.7 1.7 0 0 1-1.7 1.7H8.2a1.7 1.7 0 0 1-1.7-1.7V8.2a1.7 1.7 0 0 1 1.7-1.7z")],
        Record => p![f("M6.5 12a5.5 5.5 0 1 0 11 0a5.5 5.5 0 1 0-11 0z")],
        Recorder => p![s(CIRCLE_9), f("M8.5 12a3.5 3.5 0 1 0 7 0a3.5 3.5 0 1 0-7 0z")],
        Screen => p![s("M4.5 4.5h15A1.5 1.5 0 0 1 21 6v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 15V6a1.5 1.5 0 0 1 1.5-1.5z"), s("M12 16.5v3.5"), s("M8.5 20h7")],
        AppWindow => p![s("M5 4.5h14a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-11a2 2 0 0 1 2-2z"), s("M3 9h18"), f("M5.6 6.75a.9.9 0 1 0 1.8 0a.9.9 0 1 0-1.8 0z"), f("M8.6 6.75a.9.9 0 1 0 1.8 0a.9.9 0 1 0-1.8 0z")],
        // Four corner marks: the frame of a selection.
        Area => p![
            s("M4 8.5V5.5A1.5 1.5 0 0 1 5.5 4h3"),
            s("M15.5 4h3A1.5 1.5 0 0 1 20 5.5v3"),
            s("M20 15.5v3a1.5 1.5 0 0 1-1.5 1.5h-3"),
            s("M8.5 20h-3A1.5 1.5 0 0 1 4 18.5v-3"),
        ],
        Camera => p![s("M4 7h9.5A1.5 1.5 0 0 1 15 8.5v7a1.5 1.5 0 0 1-1.5 1.5H4a1.5 1.5 0 0 1-1.5-1.5v-7A1.5 1.5 0 0 1 4 7z"), s("M15 10.5l5-3v9l-5-3")],
        Mic => p![s("M9 6a3 3 0 0 1 6 0v5a3 3 0 0 1-6 0z"), s("M5.5 11a6.5 6.5 0 0 0 13 0"), s("M12 17.5V21")],
        Download => p![s("M12 4v10.5"), s("M7.5 10L12 14.5 16.5 10"), s("M4.5 16v2.5A1.5 1.5 0 0 0 6 20h12a1.5 1.5 0 0 0 1.5-1.5V16")],
        Stocks => p![s("M3.5 17.5l5.5-5.5 4 4 7.5-7.5"), s("M15 8.5h5.5V14")],
        Search => p![s("M4.5 10.5a6 6 0 1 0 12 0a6 6 0 1 0-12 0"), s("M15 15l4.5 4.5")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Icon] = &[
        Icon::Play, Icon::Pause, Icon::Next, Icon::Prev, Icon::Music, Icon::Lyrics, Icon::Timer, Icon::Coffee, Icon::Steps,
        Icon::Tray, Icon::ListChecks, Icon::Check, Icon::CheckCircle, Icon::Circle, Icon::Plus, Icon::Minus,
        Icon::Close, Icon::Clipboard, Icon::Copy, Icon::Calendar, Icon::Clock, Icon::Cpu, Icon::Memory, Icon::Disk,
        Icon::Battery, Icon::Bolt, Icon::Bluetooth, Icon::Headphones, Icon::Volume, Icon::VolumeLow, Icon::VolumeMute,
        Icon::Pin, Icon::PinFill, Icon::Folder, Icon::File, Icon::Trash, Icon::Undo, Icon::Reset,
        Icon::Sliders, Icon::Open, Icon::Phone, Icon::Eye, Icon::MoneyIn, Icon::MoneyOut, Icon::Transfer, Icon::Card, Icon::Wallet,
        Icon::Unlink, Icon::Gear,
        Icon::Gauge, Icon::App, Icon::Windows, Icon::Addons, Icon::Moon, Icon::Stop, Icon::Script, Icon::Record, Icon::Download,
        Icon::Stocks, Icon::Search,
        Icon::Recorder, Icon::Screen, Icon::AppWindow, Icon::Area, Icon::Camera, Icon::Mic,
    ];

    #[test]
    fn every_icon_parses() {
        for &i in ALL {
            for p in parts(i) {
                poni_core::svgpath::parse(p.d).unwrap_or_else(|e| panic!("{i:?}: {}", e.0));
            }
        }
    }
}
