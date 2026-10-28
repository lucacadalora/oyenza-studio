//! yt-dlp, the Downloader add-on's engine (`addons::DOWNLOADER`): the
//! arguments the island runs it with, and what each line it prints means.
//! Pure, so both are tested here against yt-dlp's real output
//! (`tests/fixtures/ytdlp/`). The Windows app runs it (`services::downloader`).

/// Video (picture and sound) or the sound alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Video,
    Audio,
}

/// One download the person asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub url: String,
    pub mode: Mode,
    /// The tallest picture wanted (None: the best there is).
    pub max_height: Option<u32>,
    /// The folder the finished file goes in.
    pub folder: String,
}

/// Where the add-on's tools are on this PC.
#[derive(Clone, Copy, Debug)]
pub struct Tools<'a> {
    pub ytdlp: &'a str,
    pub qjs: &'a str,
    pub ffmpeg: &'a str,
}

/// What one line of yt-dlp's output says.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// What is being downloaded (once per item, before its first byte).
    Info { title: String, site: String, thumbnail: Option<String>, duration_s: Option<f64> },
    /// Bytes so far of the part being fetched (`total` None while unknown).
    Progress { done: u64, total: Option<u64>, speed: Option<f64>, eta_s: Option<u64> },
    /// A step after the bytes are in.
    Stage(Stage),
    /// The finished file.
    Done { path: String },
    /// yt-dlp gave up on this item.
    Failed(Problem),
}

/// The steps after downloading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Joining the picture and the sound into one file.
    Merging,
    /// Turning the sound into another format.
    Converting,
    /// Tidying the file (container fixes, metadata).
    Fixing,
}

/// Why a download failed, in the words the page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// No video or sound yt-dlp knows how to read at this link.
    Unsupported,
    /// The page or video isn't there (any more).
    NotFound,
    /// Private, or only for members.
    Private,
    /// The site wants a signed-in person (YouTube's "confirm you're not a bot").
    SignIn,
    /// Not shown in this country.
    Region,
    /// The site is limiting this network for now.
    Limited,
    /// No connection, or it dropped.
    Network,
    /// The site's copy protection (DRM): it can't be saved.
    Protected,
    /// Anything else, in yt-dlp's own words (trimmed).
    Other(String),
}

impl Problem {
    /// The line the Downloads page shows for it.
    pub fn message(&self) -> String {
        todo!("dl-ytdlp")
    }
}

/// The arguments for `req` (the program itself not included).
pub fn args(req: &Request, tools: &Tools) -> Vec<String> {
    let _ = (req, tools);
    todo!("dl-ytdlp")
}

/// What `line` (one line of yt-dlp's stdout or stderr, without its line
/// ending) says; None for lines the island doesn't need.
pub fn parse(line: &str) -> Option<Line> {
    let _ = line;
    todo!("dl-ytdlp")
}
