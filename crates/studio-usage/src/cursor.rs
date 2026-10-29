//! Cursor: the plan's included usage this billing month, from cursor.com,
//! with the sign-in the Cursor app keeps on this computer.
//!
//! The sign-in sits in Cursor's VS Code-style store,
//! `<config>/Cursor/User/globalStorage/state.vscdb`: a SQLite database whose
//! `ItemTable` holds `cursorAuth/accessToken` (a JWT),
//! `cursorAuth/cachedEmail` and `cursorAuth/stripeMembershipType`. Cursor
//! keeps its chats in the same file, so it can pass 10 GB, and while Cursor
//! runs the newest pages sit in its write-ahead log (`state.vscdb-wal`). So
//! it is never read whole: [`sqlite`] walks the B-trees a page at a time,
//! about ten pieces of 4 KB for the three keys, each a [`Piece`] the app reads
//! (the contract's proposed `Want::Range` and `Step::Read`, see the lane
//! notes). Every call walks again over what was read so far, so nothing is
//! kept between steps.
//!
//! Online, as CodexBar's Cursor provider asks (MIT):
//! `GET cursor.com/api/usage-summary` (included usage, its Auto and API
//! parts, extra spend, the billing month) and `GET cursor.com/api/usage`
//! (request counts, for the older request-based plans), both with the cookie
//! Cursor's own website uses. The token goes only there, and it is never
//! renewed here: Cursor renews it when it runs.

use super::{join, jwt_claims, mask_email, num, reply_json, time, Ctx, Dirs, Money, NetFail, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Spend, Step, Trouble, Want, Window, WindowKind};
use serde_json::Value;
use std::path::PathBuf;

pub struct Cursor;

/// What is read from `ItemTable`: the sign-in, who it is, which plan.
const KEYS: [&str; 3] = ["cursorAuth/accessToken", "cursorAuth/cachedEmail", "cursorAuth/stripeMembershipType"];
const SITE: &str = "https://cursor.com";

/// Cursor's store, under `%APPDATA%` / `~/Library/Application Support`, or
/// under `VSCODE_APPDATA` when that is set (Cursor is a VS Code fork and
/// keeps its data there too).
pub fn store(dirs: &Dirs) -> PathBuf {
    let root = dirs.var("VSCODE_APPDATA").map(PathBuf::from).unwrap_or_else(|| dirs.config.clone());
    join(&root, &["Cursor", "User", "globalStorage", "state.vscdb"])
}

/// `len` bytes of a file from `offset` (fewer at its end): one
/// [`Want::Range`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub path: PathBuf,
    pub offset: u64,
    pub len: u32,
}

impl Piece {
    pub fn want(self) -> Want {
        Want::Range { path: self.path, offset: self.offset, len: self.len }
    }
}

/// What the reader does next: read more pieces first ([`Step::Read`]: the
/// app appends what it read to `Ctx::got` and asks again), or one of the
/// contract's other steps.
pub enum Go {
    Read(Vec<Piece>),
    Step(Step),
}

impl Cursor {
    /// The first piece to read: the start of the store, its header and (at
    /// the usual 4 KB page) its first page.
    pub fn first(dirs: &Dirs) -> Vec<Piece> {
        vec![Piece { path: store(dirs), offset: 0, len: sqlite::FIRST }]
    }

    /// The next step, from the pieces read so far (`cx.got`, in the order
    /// they were asked for, [`Cursor::first`]'s first) and the answers.
    pub fn go(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Go {
        let mut reads = sqlite::Reads::new(cx.got);
        let values = match sqlite::lookup(&mut reads, "ItemTable", &KEYS) {
            Ok(values) => values,
            Err(sqlite::Halt::Need(part, offset, len)) => return Go::Read(vec![Piece { path: part.path(&store(cx.dirs)), offset, len }]),
            Err(sqlite::Halt::Missing) => return Go::Step(Step::Done(Err(Trouble::NotFound))),
            Err(sqlite::Halt::Bad(why)) => return Go::Step(Step::Done(Err(Trouble::Unreadable(why.into())))),
        };
        Go::Step(online(cx, &values, replies))
    }
}

impl Provider for Cursor {
    fn id(&self) -> ProviderId {
        ProviderId::Cursor
    }

    fn env(&self) -> &'static [&'static str] {
        &["VSCODE_APPDATA"]
    }

    fn wants(&self, dirs: &Dirs) -> Vec<Want> {
        Cursor::first(dirs).into_iter().map(Piece::want).collect()
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        match self.go(cx, replies) {
            Go::Step(step) => step,
            Go::Read(pieces) => Step::Read(pieces.into_iter().map(Piece::want).collect()),
        }
    }

    /// Cursor's numbers move with use, billed by the month: every 5 minutes
    /// is plenty.
    fn min_gap_s(&self) -> u64 {
        300
    }
}

/// From the store's values (`KEYS`, in order) to the asks and the snapshot.
fn online(cx: &Ctx, values: &[Option<String>], replies: &[Result<Reply, NetFail>]) -> Step {
    let token = match values[0].as_deref().map(str::trim) {
        Some(t) if !t.is_empty() => t,
        _ => return Step::Done(Err(Trouble::SignedOut)),
    };
    let Some(sign) = Sign::read(token) else {
        return Step::Done(Err(Trouble::Unreadable("Cursor's sign-in isn't in a form this reads".into())));
    };
    // CodexBar's margin: a token this close to its end can lapse on the way.
    if sign.exp <= cx.now + 60 {
        return Step::Done(Err(Trouble::Expired));
    }
    if cx.reach == Reach::Local {
        return Step::Done(Err(Trouble::NeedsOnline));
    }
    // The cookie cursor.com's own pages send: "<user id>::<token>", the colons escaped.
    let ask = |url: String| Request::get(url).header("Accept", "application/json").header("Cookie", format!("WorkosCursorSessionToken={}%3A%3A{token}", sign.user));
    let Some(first) = replies.first() else {
        return Step::Fetch(ask(format!("{SITE}/api/usage-summary")));
    };
    let summary = match reply_json(first) {
        Ok(v) => v,
        Err(t) => return Step::Done(Err(t)),
    };
    if !["individualUsage", "membershipType", "billingCycleEnd"].iter().any(|k| summary.get(k).is_some_and(|v| !v.is_null())) {
        return Step::Done(Err(Trouble::Unreadable("Cursor's answer has no usage in it".into())));
    }
    match replies.get(1) {
        // Request counts: the older request-based plans have them, the rest
        // get an empty answer, and a failure here hides nothing.
        None => Step::Fetch(ask(format!("{SITE}/api/usage?user={}", sign.user))),
        Some(r) => Step::Done(Ok(snapshot(cx, values, &sign, &summary, reply_json(r).ok().as_ref()))),
    }
}

/// What the token says about itself, unverified (only cursor.com checks it).
struct Sign {
    /// The id in the cookie: the last part of the `sub` claim
    /// ("auth0|user_01..." -> "user_01...").
    user: String,
    exp: i64,
    email: Option<String>,
}

impl Sign {
    fn read(token: &str) -> Option<Sign> {
        // A JWT's alphabet, which also keeps the cookie header clean.
        if !token.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')) {
            return None;
        }
        let claims = jwt_claims(token)?;
        let user = claims["sub"].as_str()?.split('|').rfind(|p| !p.is_empty())?;
        if !user.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')) {
            return None;
        }
        Some(Sign { user: user.into(), exp: time(&claims["exp"])?, email: claims["email"].as_str().map(str::to_string) })
    }
}

/// The card: the plan, who, the month's usage and extra spend.
fn snapshot(cx: &Ctx, values: &[Option<String>], sign: &Sign, s: &Value, requests: Option<&Value>) -> Snapshot {
    let mut snap = Snapshot::new(ProviderId::Cursor, cx.now, Source::Online);
    fn filled(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|v| !v.is_empty())
    }
    snap.plan = filled(s["membershipType"].as_str()).or(filled(values[2].as_deref())).map(plan_name);
    snap.account = filled(values[1].as_deref()).or(filled(sign.email.as_deref())).map(mask_email);
    let resets = time(&s["billingCycleEnd"]);
    let length = match (time(&s["billingCycleStart"]), resets) {
        (Some(a), Some(b)) if b > a => u32::try_from(b - a).ok(),
        _ => None,
    };
    let month = |label: &str, used: f32| Window::new(WindowKind::Monthly, label, used).resets(resets).length(length);
    let usage = &s["individualUsage"];
    let plan = &usage["plan"];
    if let Some((used, of)) = request_count(requests) {
        // A request-based plan: its count is the plan (the Auto and API
        // parts belong to the usage-based plans).
        snap.windows.push(month("Requests", used as f32 / of as f32).counted(used, of));
    } else {
        // Percent units already: 0.36 is 0.36 %, not 36 %.
        let pct = |v: &Value| num(v).map(|p| (p / 100.0) as f32);
        let (auto, api) = (pct(&plan["autoPercentUsed"]), pct(&plan["apiPercentUsed"]));
        // CodexBar's order when the total is missing: the two parts (the
        // dashboard's blend is their mean), then cents of a limit: the plan's,
        // a team member's own cap, the team's shared pool.
        let total = pct(&plan["totalPercentUsed"])
            .or(match (auto, api) {
                (Some(a), Some(b)) => Some((a + b) / 2.0),
                (a, b) => b.or(a),
            })
            .or_else(|| share(&plan["used"], &plan["limit"]))
            .or_else(|| share(&usage["overall"]["used"], &usage["overall"]["limit"]))
            .or_else(|| share(&s["teamUsage"]["pooled"]["used"], &s["teamUsage"]["pooled"]["limit"]));
        if let Some(t) = total {
            snap.windows.push(month("Monthly", t));
        }
        if let Some(a) = auto {
            snap.windows.push(month("Monthly · Auto", a));
        }
        if let Some(a) = api {
            snap.windows.push(month("Monthly · API", a));
        }
    }
    snap.spend.extend(extra(usage, &s["teamUsage"]));
    snap
}

/// (used, of) from `/api/usage`, when the plan counts requests.
fn request_count(r: Option<&Value>) -> Option<(u64, u64)> {
    let g = &r?["gpt-4"];
    let of = num(&g["maxRequestUsage"]).filter(|n| *n >= 1.0)?;
    let used = num(&g["numRequestsTotal"]).or_else(|| num(&g["numRequests"]))?;
    Some((used.max(0.0) as u64, of as u64))
}

/// `used / limit` (both cents), when there is a limit.
fn share(used: &Value, limit: &Value) -> Option<f32> {
    let limit = num(limit).filter(|l| *l > 0.0)?;
    Some((num(used)? / limit) as f32)
}

/// Spend past the plan (Cursor's "on-demand" use), against the person's own
/// cap, else the team's shared one.
fn extra(usage: &Value, team: &Value) -> Option<Spend> {
    let usd = |v: &Value| num(v).map(|cents| Money::usd(cents / 100.0));
    let cap = |v: &Value| usd(&v["limit"]).filter(|m| m.micros > 0);
    let (mine, team) = (&usage["onDemand"], &team["onDemand"]);
    let (label, used, cap) = match (cap(mine), cap(team)) {
        (Some(c), _) => ("Extra this month", usd(&mine["used"]), Some(c)),
        (None, Some(c)) => ("Team extra this month", usd(&team["used"]), Some(c)),
        (None, None) => ("Extra this month", usd(&mine["used"]).filter(|m| m.micros > 0), None),
    };
    Some(Spend { label: label.into(), money: used?, cap })
}

/// The membership type as people know the plan (CodexBar's names).
fn plan_name(kind: &str) -> String {
    let name = match kind.to_ascii_lowercase().as_str() {
        "free" => "Free",
        "free_trial" => "Pro trial",
        "hobby" => "Hobby",
        "pro" | "pro_student" => "Pro",
        "pro_plus" => "Pro+",
        "ultra" => "Ultra",
        "team" => "Team",
        "business" => "Business",
        "enterprise" => "Enterprise",
        "express" => "Start",
        _ => {
            let words = kind.replace('_', " ");
            let mut chars = words.chars();
            return chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default();
        }
    };
    name.into()
}

/// A read-only reader for a few keys of one key/value table in a SQLite
/// database, a page at a time, for files far too big to read whole
/// (sqlite.org/fileformat2.html: B-trees, overflow pages, records;
/// sqlite.org/walformat.html: the write-ahead log and its index, the `-shm`
/// file). It never takes SQLite's locks: it checks what it reads instead
/// (the index header's checksum, each frame's page number and salts, the
/// key of the row it lands on), and a database that changes under it reads
/// as [`Halt::Bad`] until the next refresh.
mod sqlite {
    use super::super::Got;
    use std::borrow::Cow;
    use std::cmp::Ordering;
    use std::path::{Path, PathBuf};

    /// The first piece: the header, and page 1 at the usual 4 KB page (at
    /// 512 bytes, the first eight pages).
    pub const FIRST: u32 = 4096;
    /// Pages one lookup may visit, read or not: a loop in a damaged file
    /// ends here.
    const VISITS: u32 = 256;
    /// B-tree levels (a 10 GB database has about four).
    const DEPTH: usize = 20;
    /// The largest value read: a sign-in token is about 1 KB.
    const VALUE_MAX: u64 = 64 * 1024;
    /// WAL frames looked through: 64 index blocks, about 1 GB of WAL at 4 KB
    /// pages (a busy Cursor's has been seen near 90 MB).
    const FRAMES_MAX: u32 = 4062 + 63 * 4096;
    const ODD: Halt = Halt::Bad("Cursor's sign-in file has a layout this can't read");
    const CHANGED: Halt = Halt::Bad("Cursor's sign-in file changed while being read");

    /// The database's three files.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Part {
        Db,
        Wal,
        Shm,
    }

    impl Part {
        pub fn path(self, db: &Path) -> PathBuf {
            let mut p = db.as_os_str().to_owned();
            match self {
                Part::Db => {}
                Part::Wal => p.push("-wal"),
                Part::Shm => p.push("-shm"),
            }
            PathBuf::from(p)
        }
    }

    /// Why a lookup stopped.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Halt {
        /// This piece of this file isn't read yet: read it and walk again.
        Need(Part, u64, u32),
        /// The database file isn't there.
        Missing,
        /// Not a database this reads, or it changed while it was read.
        Bad(&'static str),
    }

    /// The pieces read so far, in the order the walk asked for them. A piece
    /// inside one already read is cut from it, so walking again reads
    /// nothing new.
    pub struct Reads<'a> {
        got: &'a [Got],
        used: usize,
        /// (file, offset, length asked, index in `got`).
        seen: Vec<(Part, u64, u32, usize)>,
        visits: u32,
    }

    impl<'a> Reads<'a> {
        pub fn new(got: &'a [Got]) -> Reads<'a> {
            Reads { got, used: 0, seen: Vec::new(), visits: 0 }
        }

        /// `offset..offset + len` of a file (shorter at its end), None if the
        /// file isn't there.
        fn read(&mut self, part: Part, offset: u64, len: u32) -> Result<Option<&'a [u8]>, Halt> {
            let end = offset + len as u64;
            let (i, skip) = match self.seen.iter().find(|s| s.0 == part && s.1 <= offset && end <= s.1 + s.2 as u64) {
                Some(s) => (s.3, (offset - s.1) as usize),
                None if self.used < self.got.len() => {
                    self.seen.push((part, offset, len, self.used));
                    self.used += 1;
                    (self.used - 1, 0)
                }
                None => return Err(Halt::Need(part, offset, len)),
            };
            let got = self.got;
            Ok(match &got[i] {
                Got::One(f) => {
                    let start = skip.min(f.bytes.len());
                    Some(&f.bytes[start..(start + len as usize).min(f.bytes.len())])
                }
                _ => None,
            })
        }
    }

    /// The values of `keys` in `table`, a key/value table like VS Code's
    /// `ItemTable (key TEXT UNIQUE, value BLOB)`: found through the key's
    /// index, then the row, whose key must match again. A key that isn't
    /// there (or a table not made yet) is None. UTF-8 databases only (the
    /// only kind VS Code and Cursor make).
    pub fn lookup(r: &mut Reads, table: &str, keys: &[&str]) -> Result<Vec<Option<String>>, Halt> {
        let none = vec![None; keys.len()];
        let Some(mut db) = Db::open(r)? else { return Ok(none) };
        let (Some(t), index) = db.roots(table)? else { return Ok(none) };
        let index = index.ok_or(ODD)?;
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let k = key.as_bytes();
            let found = match db.seek(index, k)? {
                Some((entry, cols)) => {
                    let rowid = int(&entry, cols[1]).ok_or(ODD)?;
                    let (row, cols) = db.row(t, rowid)?.ok_or(CHANGED)?;
                    if text_bytes(&row, cols[0]) != Some(k) {
                        return Err(CHANGED);
                    }
                    Some((row, cols))
                }
                None => None,
            };
            out.push(found.and_then(|(rec, cols)| text(&rec, cols[1])));
        }
        Ok(out)
    }

    /// An open database: its page size, and the WAL's frames.
    struct Db<'r, 'a> {
        r: &'r mut Reads<'a>,
        size: u32,
        /// Bytes of each page that hold the B-tree (the rest is reserved).
        usable: usize,
        wal: Option<Wal<'a>>,
    }

    /// The write-ahead log as its index saw it: the page number of each frame
    /// up to the last one committed.
    struct Wal<'a> {
        salt: &'a [u8],
        /// The `-shm` file from byte 136, through the last frame's slot.
        slots: &'a [u8],
        last: u32,
    }

    impl Wal<'_> {
        /// The newest frame holding page `n`.
        fn frame(&self, n: u32) -> Option<u32> {
            (1..=self.last).rev().find(|&f| le32(self.slots, slot(f) - 136) == Some(n))
        }
    }

    /// Where frame `f`'s page number sits in the `-shm` file: 4062 slots in
    /// the first 32 KB block (after its 136-byte header), 4096 in each next.
    fn slot(f: u32) -> usize {
        let f = f as usize;
        if f <= 4062 {
            136 + 4 * (f - 1)
        } else {
            let i = f - 4063;
            (i / 4096 + 1) * 32768 + 4 * (i % 4096)
        }
    }

    /// A B-tree page: its kind (2 and 5 interior index and table pages, 10
    /// and 13 leaves) and where its cell pointers are.
    struct Node<'a> {
        b: &'a [u8],
        kind: u8,
        cells: usize,
        ptrs: usize,
        right: u32,
    }

    /// The part of a cell's payload on its page, the payload's whole size and
    /// the first overflow page when the rest spills over.
    struct Payload<'a> {
        here: &'a [u8],
        size: u64,
        next: u32,
    }

    /// A record's column: serial type, where it starts, its length.
    #[derive(Clone, Copy)]
    struct Col {
        kind: u64,
        at: usize,
        len: usize,
    }

    type Record<'a> = (Cow<'a, [u8]>, Vec<Col>);

    impl<'r, 'a> Db<'r, 'a> {
        /// The header, and the WAL's index when the database is in WAL mode.
        /// None for an empty file (a database nothing was written to yet).
        fn open(r: &'r mut Reads<'a>) -> Result<Option<Db<'r, 'a>>, Halt> {
            let h = r.read(Part::Db, 0, FIRST)?.ok_or(Halt::Missing)?;
            if h.is_empty() {
                return Ok(None);
            }
            if h.len() < 100 || &h[..16] != b"SQLite format 3\0" {
                return Err(Halt::Bad("Cursor's sign-in file isn't a database"));
            }
            let size = match be16(h, 16) {
                Some(1) => 65536,
                Some(s) => s as u32,
                None => 0,
            };
            let usable = (size as usize).saturating_sub(h[20] as usize);
            // Text encoding 1 is UTF-8.
            if !size.is_power_of_two() || size < 512 || usable < 480 || be32(h, 56) != Some(1) {
                return Err(ODD);
            }
            // Bytes 18 and 19 are 2 in WAL mode, which stays set after Cursor
            // closes; without a -shm the main file is the whole database.
            let wal = if h[18] == 2 { wal(r, size)? } else { None };
            Ok(Some(Db { r, size, usable, wal }))
        }

        /// Page `n` as the database has it now: its newest WAL frame, else the
        /// main file.
        fn page(&mut self, n: u32) -> Result<&'a [u8], Halt> {
            self.r.visits += 1;
            if n == 0 || self.r.visits > VISITS {
                return Err(ODD);
            }
            let size = self.size as usize;
            if let Some(w) = &self.wal {
                if let Some(f) = w.frame(n) {
                    let salt = w.salt;
                    let b = self.r.read(Part::Wal, 32 + (f as u64 - 1) * (24 + size as u64), 24 + self.size)?.ok_or(CHANGED)?;
                    // Each frame names its page and the salts of its WAL: a
                    // frame Cursor rewrote since the index was read differs.
                    if b.len() != 24 + size || be32(b, 0) != Some(n) || &b[8..16] != salt {
                        return Err(CHANGED);
                    }
                    return Ok(&b[24..]);
                }
            }
            let b = self.r.read(Part::Db, (n as u64 - 1) * size as u64, self.size)?.ok_or(CHANGED)?;
            if b.len() != size {
                return Err(CHANGED);
            }
            Ok(b)
        }

        fn node(&mut self, n: u32) -> Result<Node<'a>, Halt> {
            let b = self.page(n)?;
            // Page 1 starts with the 100-byte file header.
            let at = if n == 1 { 100 } else { 0 };
            let kind = b[at];
            let leaf = match kind {
                2 | 5 => false,
                10 | 13 => true,
                _ => return Err(ODD),
            };
            let cells = be16(b, at + 3).ok_or(ODD)? as usize;
            let ptrs = at + if leaf { 8 } else { 12 };
            let right = if leaf { 0 } else { be32(b, at + 8).ok_or(ODD)? };
            if ptrs + 2 * cells > self.usable {
                return Err(ODD);
            }
            Ok(Node { b, kind, cells, ptrs, right })
        }

        /// Where cell `i` starts on its page.
        fn cell(&self, node: &Node, i: usize) -> Result<usize, Halt> {
            let at = be16(node.b, node.ptrs + 2 * i).ok_or(ODD)? as usize;
            if at < node.ptrs + 2 * node.cells || at >= self.usable {
                return Err(ODD);
            }
            Ok(at)
        }

        /// Table cell `i`: its rowid, and its child page (interior) or its
        /// payload's size and start (leaf).
        fn table_cell(&self, node: &Node, i: usize) -> Result<(i64, u64, usize), Halt> {
            let at = self.cell(node, i)?;
            if node.kind == 5 {
                let child = be32(node.b, at).ok_or(ODD)?;
                let (key, _) = varint(node.b, at + 4).ok_or(ODD)?;
                return Ok((key as i64, child as u64, 0));
            }
            let (size, at) = varint(node.b, at).ok_or(ODD)?;
            let (rowid, at) = varint(node.b, at).ok_or(ODD)?;
            Ok((rowid as i64, size, at))
        }

        /// The payload of `size` bytes starting at `at`: how much stays on the
        /// page is SQLite's rule for table leaves and for index cells.
        fn payload(&self, node: &Node<'a>, at: usize, size: u64) -> Result<Payload<'a>, Halt> {
            let u = self.usable as u64;
            let max = if node.kind == 13 { u - 35 } else { (u - 12) * 64 / 255 - 23 };
            let min = (u - 12) * 32 / 255 - 23;
            let here = if size <= max {
                size
            } else {
                let k = min + (size - min) % (u - 4);
                if k <= max { k } else { min }
            } as usize;
            let end = at.checked_add(here).filter(|&e| e <= self.usable).ok_or(ODD)?;
            let next = if (here as u64) < size { be32(node.b, end).filter(|_| end + 4 <= self.usable).ok_or(ODD)? } else { 0 };
            Ok(Payload { here: &node.b[at..end], size, next })
        }

        /// The whole payload, following its overflow pages.
        fn whole(&mut self, p: &Payload<'a>) -> Result<Cow<'a, [u8]>, Halt> {
            if p.next == 0 {
                return Ok(Cow::Borrowed(p.here));
            }
            if p.size > VALUE_MAX {
                return Err(Halt::Bad("a value in Cursor's sign-in file is too big"));
            }
            let size = p.size as usize;
            let mut out = p.here.to_vec();
            let mut next = p.next;
            while out.len() < size {
                // An overflow page: the next one's number, then content.
                let page = self.page(next)?;
                let take = (size - out.len()).min(self.usable - 4);
                out.extend_from_slice(page.get(4..4 + take).ok_or(ODD)?);
                next = be32(page, 0).ok_or(ODD)?;
                if next == 0 && out.len() < size {
                    return Err(ODD);
                }
            }
            Ok(Cow::Owned(out))
        }

        /// A payload's record with its first `want` columns readable, reading
        /// overflow pages only when one of those spills onto them.
        fn record(&mut self, p: &Payload<'a>, want: usize) -> Result<Record<'a>, Halt> {
            if let Some(cols) = columns(p.here, want)? {
                return Ok((Cow::Borrowed(p.here), cols));
            }
            let whole = self.whole(p)?;
            let cols = columns(&whole, want)?.ok_or(ODD)?;
            Ok((whole, cols))
        }

        /// Index cell `i`'s record, with `want` columns readable.
        fn entry(&mut self, node: &Node<'a>, i: usize, want: usize) -> Result<Record<'a>, Halt> {
            let mut at = self.cell(node, i)?;
            if node.kind == 2 {
                at += 4; // the left child's page number
            }
            let (size, at) = varint(node.b, at).ok_or(ODD)?;
            let p = self.payload(node, at, size)?;
            self.record(&p, want)
        }

        /// The entry for `key` in the index B-tree at `root`: (key, rowid).
        fn seek(&mut self, root: u32, key: &[u8]) -> Result<Option<Record<'a>>, Halt> {
            let mut n = root;
            for _ in 0..DEPTH {
                let node = self.node(n)?;
                if node.kind != 2 && node.kind != 10 {
                    return Err(ODD);
                }
                let (mut lo, mut hi) = (0, node.cells);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let (rec, cols) = self.entry(&node, mid, 1)?;
                    match order(key, &rec, cols[0])? {
                        Ordering::Less => hi = mid,
                        Ordering::Greater => lo = mid + 1,
                        Ordering::Equal => return self.entry(&node, mid, 2).map(Some),
                    }
                }
                if node.kind == 10 {
                    return Ok(None);
                }
                // Keys before cell `lo`'s sit in its left child; past the last
                // cell, in the right-most page.
                n = if lo < node.cells { be32(node.b, self.cell(&node, lo)?).ok_or(ODD)? } else { node.right };
            }
            Err(ODD)
        }

        /// The row `rowid` in the table B-tree at `root`, key and value readable.
        fn row(&mut self, root: u32, rowid: i64) -> Result<Option<Record<'a>>, Halt> {
            let mut n = root;
            for _ in 0..DEPTH {
                let node = self.node(n)?;
                if node.kind != 5 && node.kind != 13 {
                    return Err(ODD);
                }
                // The first cell whose rowid is at least `rowid`: in an
                // interior page its child holds every rowid up to its own.
                let (mut lo, mut hi) = (0, node.cells);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if self.table_cell(&node, mid)?.0 < rowid {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                let cell = if lo < node.cells { Some(self.table_cell(&node, lo)?) } else { None };
                if node.kind == 13 {
                    return match cell {
                        Some((id, size, at)) if id == rowid => {
                            let p = self.payload(&node, at, size)?;
                            self.record(&p, 2).map(Some)
                        }
                        _ => Ok(None),
                    };
                }
                n = match cell {
                    Some((_, child, _)) => child as u32,
                    None => node.right,
                };
            }
            Err(ODD)
        }

        /// The root pages of `table` and of its key's index (the one SQLite
        /// makes for `UNIQUE` or `PRIMARY KEY`), from `sqlite_schema`.
        fn roots(&mut self, table: &str) -> Result<(Option<u32>, Option<u32>), Halt> {
            let (name, index) = (table.as_bytes(), format!("sqlite_autoindex_{table}_1"));
            let (mut t, mut x) = (None, None);
            let mut todo = vec![1];
            while let Some(n) = todo.pop() {
                let node = self.node(n)?;
                if node.kind != 5 && node.kind != 13 {
                    return Err(ODD);
                }
                for i in 0..node.cells {
                    // An interior cell's child page, or a leaf cell's payload.
                    let (_, child_or_size, at) = self.table_cell(&node, i)?;
                    if node.kind == 5 {
                        todo.push(child_or_size as u32);
                        continue;
                    }
                    // (type, name, tbl_name, rootpage, sql)
                    let p = self.payload(&node, at, child_or_size)?;
                    let (rec, c) = self.record(&p, 4)?;
                    let root = int(&rec, c[3]).and_then(|r| u32::try_from(r).ok()).filter(|r| *r > 0);
                    let (kind, own, of) = (text_bytes(&rec, c[0]), text_bytes(&rec, c[1]), text_bytes(&rec, c[2]));
                    if kind == Some(b"table") && own == Some(name) {
                        t = root;
                    } else if kind == Some(b"index") && of == Some(name) && own == Some(index.as_bytes()) {
                        x = root;
                    }
                }
                if node.kind == 5 {
                    todo.push(node.right);
                }
            }
            Ok((t, x))
        }
    }

    /// The WAL's index: its header (two copies, 96 bytes; never bytes 120 to
    /// 128, which Windows won't let another process read while SQLite holds
    /// its locks there), then the page number of each frame up to the last
    /// one committed. No `-shm`, or nothing in it: the main file is the whole
    /// database.
    fn wal<'a>(r: &mut Reads<'a>, size: u32) -> Result<Option<Wal<'a>>, Halt> {
        let h = match r.read(Part::Shm, 0, 96)? {
            Some(h) if h.len() == 96 => h,
            _ => return Ok(None),
        };
        let (a, b) = h.split_at(48);
        // SQLite writes the second copy first, so two different copies mean
        // a write in progress.
        if a != b {
            return Err(CHANGED);
        }
        // Not set up yet: SQLite rebuilds it from the WAL on its next open.
        if a[12] == 0 {
            return Ok(None);
        }
        let (s1, s2) = checksum(&a[..40]);
        if le32(a, 40) != Some(s1) || le32(a, 44) != Some(s2) {
            return Err(CHANGED);
        }
        let page = le16(a, 14).unwrap_or(0) as u32;
        if le32(a, 0) != Some(3_007_000) || (page & 0xfe00) + ((page & 1) << 16) != size {
            return Err(ODD);
        }
        let last = le32(a, 16).unwrap_or(0);
        if last == 0 {
            return Ok(None);
        }
        if last > FRAMES_MAX {
            return Err(Halt::Bad("Cursor's sign-in file has too many recent changes to read"));
        }
        let len = slot(last) + 4 - 136;
        let slots = r.read(Part::Shm, 136, len as u32)?.filter(|s| s.len() == len).ok_or(CHANGED)?;
        Ok(Some(Wal { salt: &h[32..40], slots, last }))
    }

    /// SQLite's checksum of the index header, in this computer's byte order
    /// (little-endian on every Windows and Mac this runs on).
    fn checksum(b: &[u8]) -> (u32, u32) {
        b.as_chunks::<8>().0.iter().fold((0u32, 0u32), |(s1, s2), c| {
            let s1 = s1.wrapping_add(le32(c, 0).unwrap_or(0)).wrapping_add(s2);
            let s2 = s2.wrapping_add(le32(c, 4).unwrap_or(0)).wrapping_add(s1);
            (s1, s2)
        })
    }

    /// The first `want` columns of a record. None when the record goes on
    /// past `rec` (onto overflow pages).
    fn columns(rec: &[u8], want: usize) -> Result<Option<Vec<Col>>, Halt> {
        let Some((head, mut i)) = varint(rec, 0) else { return Ok(None) };
        let head = usize::try_from(head).map_err(|_| ODD)?;
        let mut at = head;
        let mut cols = Vec::with_capacity(want);
        while cols.len() < want {
            if i >= head {
                return Err(ODD);
            }
            let Some((kind, next)) = varint(rec, i) else { return Ok(None) };
            i = next;
            let len = match kind {
                0 | 8 | 9 => 0,
                1..=4 => kind as usize,
                5 => 6,
                6 | 7 => 8,
                10 | 11 => return Err(ODD),
                k => usize::try_from((k - 12) / 2).map_err(|_| ODD)?,
            };
            cols.push(Col { kind, at, len });
            at = at.checked_add(len).ok_or(ODD)?;
        }
        Ok(if at <= rec.len() { Some(cols) } else { None })
    }

    /// How a TEXT `key` sorts against a record's first column, by SQLite's
    /// rules: NULL and numbers first, then TEXT by its bytes, then BLOBs.
    fn order(key: &[u8], rec: &[u8], c: Col) -> Result<Ordering, Halt> {
        Ok(match c.kind {
            0..=9 => Ordering::Greater,
            k if k % 2 == 0 => Ordering::Less,
            _ => key.cmp(rec.get(c.at..c.at + c.len).ok_or(ODD)?),
        })
    }

    fn int(rec: &[u8], c: Col) -> Option<i64> {
        match c.kind {
            8 => Some(0),
            9 => Some(1),
            1..=6 => {
                let b = rec.get(c.at..c.at + c.len)?;
                Some(b.iter().fold(if b[0] & 0x80 != 0 { -1 } else { 0 }, |v, &x| (v << 8) | x as i64))
            }
            _ => None,
        }
    }

    /// A value as text: TEXT, or a BLOB holding UTF-8 or UTF-16LE. NULL and
    /// numbers are None.
    fn text(rec: &[u8], c: Col) -> Option<String> {
        match c.kind {
            k if k >= 13 && k % 2 == 1 => String::from_utf8(rec.get(c.at..c.at + c.len)?.to_vec()).ok(),
            k if k >= 12 => blob_text(rec.get(c.at..c.at + c.len)?),
            _ => None,
        }
    }

    fn text_bytes(rec: &[u8], c: Col) -> Option<&[u8]> {
        if c.kind >= 13 && c.kind % 2 == 1 {
            rec.get(c.at..c.at + c.len)
        } else {
            None
        }
    }

    /// A BLOB as text, as CodexBar reads Cursor's: ASCII in UTF-16LE (every
    /// second byte 0, which also passes as UTF-8) first, then UTF-8, then
    /// UTF-16LE.
    pub fn blob_text(b: &[u8]) -> Option<String> {
        if !b.is_empty() && b.len().is_multiple_of(2) && b.chunks(2).all(|p| (1..128).contains(&p[0]) && p[1] == 0) {
            return utf16le(b);
        }
        std::str::from_utf8(b).map(str::to_string).ok().or_else(|| utf16le(b))
    }

    fn utf16le(b: &[u8]) -> Option<String> {
        if !b.len().is_multiple_of(2) {
            return None;
        }
        char::decode_utf16(b.chunks(2).map(|p| u16::from_le_bytes([p[0], p[1]]))).collect::<Result<String, _>>().ok()
    }

    /// SQLite's varint: 1 to 9 bytes, big-endian, 7 bits a byte (all 8 in
    /// the ninth).
    fn varint(b: &[u8], mut i: usize) -> Option<(u64, usize)> {
        let mut v = 0u64;
        for n in 0..9 {
            let c = *b.get(i)?;
            i += 1;
            if n == 8 {
                return Some(((v << 8) | c as u64, i));
            }
            v = (v << 7) | (c & 0x7f) as u64;
            if c < 0x80 {
                return Some((v, i));
            }
        }
        None
    }

    fn be16(b: &[u8], i: usize) -> Option<u16> {
        Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?))
    }

    fn be32(b: &[u8], i: usize) -> Option<u32> {
        Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
    }

    fn le16(b: &[u8], i: usize) -> Option<u16> {
        Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
    }

    fn le32(b: &[u8], i: usize) -> Option<u32> {
        Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::sqlite::{blob_text, lookup, Halt, Part, Reads};
    use super::*;
    use crate::{FileRead, Got, Os, Outcome, MAX_FETCHES};

    const STATE: &[u8] = include_bytes!("fixtures/cursor/state.vscdb");
    const WAL_DB: &[u8] = include_bytes!("fixtures/cursor/wal.vscdb");
    const WAL_LOG: &[u8] = include_bytes!("fixtures/cursor/wal.vscdb-wal");
    const WAL_SHM: &[u8] = include_bytes!("fixtures/cursor/wal.vscdb-shm");
    const SIGNED_OUT: &[u8] = include_bytes!("fixtures/cursor/signed-out.vscdb");
    const SUMMARY: &str = include_str!("fixtures/cursor/usage-summary.json");
    const REQUESTS: &str = include_str!("fixtures/cursor/usage-requests.json");
    const NO_REQUESTS: &str = include_str!("fixtures/cursor/usage-no-requests.json");
    /// 2026-10-27T05:00:00Z.
    const NOW: i64 = 1_793_077_200;
    /// The fixtures' current tokens end then (2033).
    const EXP: i64 = 2_000_000_000;

    /// The store's files for the Windows sample: the database, and its WAL
    /// and index when given.
    fn files(db: &[u8], wal: Option<(&[u8], &[u8])>) -> Vec<(PathBuf, Vec<u8>)> {
        let path = store(&Dirs::sample(Os::Windows));
        let mut out = vec![(path.clone(), db.to_vec())];
        if let Some((log, shm)) = wal {
            out.push((Part::Wal.path(&path), log.to_vec()));
            out.push((Part::Shm.path(&path), shm.to_vec()));
        }
        out
    }

    /// The app's side of a piece, as proposed for `Want::Range`.
    fn read(files: &[(PathBuf, Vec<u8>)], p: &Piece) -> Got {
        match files.iter().find(|(path, _)| *path == p.path) {
            None => Got::Missing,
            Some((_, b)) => {
                let start = (p.offset as usize).min(b.len());
                let end = (start + p.len as usize).min(b.len());
                Got::One(FileRead { path: p.path.clone(), bytes: b[start..end].to_vec(), modified: None })
            }
        }
    }

    /// Just the store's lookup, reading pieces until it is done: its result
    /// and every piece it asked for.
    fn values(files: &[(PathBuf, Vec<u8>)], keys: &[&str]) -> (Result<Vec<Option<String>>, Halt>, Vec<Piece>) {
        let db = store(&Dirs::sample(Os::Windows));
        let (mut got, mut asked) = (Vec::new(), Vec::new());
        loop {
            match lookup(&mut Reads::new(&got), "ItemTable", keys) {
                Err(Halt::Need(part, offset, len)) => {
                    let piece = Piece { path: part.path(&db), offset, len };
                    got.push(read(files, &piece));
                    asked.push(piece);
                }
                done => return (done, asked),
            }
        }
    }

    struct Run {
        outcome: Outcome,
        asked: Vec<Request>,
        pieces: usize,
    }

    /// The app's loop as proposed: the first pieces, then `Step::Read`s,
    /// fetches answered from `answers` in order.
    fn run(files: &[(PathBuf, Vec<u8>)], reach: Reach, now: i64, answers: &[Result<Reply, NetFail>]) -> Run {
        let dirs = Dirs::sample(Os::Windows);
        let mut got: Vec<Got> = Cursor::first(&dirs).iter().map(|p| read(files, p)).collect();
        let (mut replies, mut asked) = (Vec::new(), Vec::new());
        loop {
            let cx = Ctx { dirs: &dirs, got: &got, now, reach };
            match Cursor.go(&cx, &replies) {
                Go::Step(Step::Read(w)) => panic!("Cursor reads through Go::Read, not Step::Read: {w:?}"),
                Go::Read(pieces) => {
                    assert!(got.len() + pieces.len() <= 64, "too many pieces");
                    got.extend(pieces.iter().map(|p| read(files, p)));
                }
                Go::Step(Step::Fetch(r)) => {
                    assert_eq!(reach, Reach::Online, "fetched on a local refresh");
                    assert!(replies.len() < MAX_FETCHES);
                    asked.push(r);
                    replies.push(answers.get(replies.len()).cloned().expect("an answer for every fetch"));
                }
                Go::Step(Step::Done(outcome)) => return Run { outcome, asked, pieces: got.len() },
            }
        }
    }

    fn ok(body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status: 200, body: body.into(), retry_after_s: None })
    }

    fn status(status: u16, retry_after_s: Option<u64>) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: "{}".into(), retry_after_s })
    }

    fn exp_of(token: &str) -> i64 {
        Sign::read(token).unwrap().exp
    }

    #[test]
    fn paths_on_windows_and_mac() {
        let win = Dirs::sample(Os::Windows);
        assert_eq!(Cursor::first(&win), vec![Piece { path: win.config.join("Cursor").join("User").join("globalStorage").join("state.vscdb"), offset: 0, len: 4096 }]);
        let mac = Dirs::sample(Os::Mac);
        let path = store(&mac);
        assert_eq!(path, PathBuf::from("/Users/u/Library/Application Support/Cursor/User/globalStorage/state.vscdb"));
        assert_eq!(Part::Wal.path(&path), PathBuf::from("/Users/u/Library/Application Support/Cursor/User/globalStorage/state.vscdb-wal"));
        assert_eq!(Part::Shm.path(&path), PathBuf::from("/Users/u/Library/Application Support/Cursor/User/globalStorage/state.vscdb-shm"));
        let mut moved = Dirs::sample(Os::Mac);
        moved.env.push(("VSCODE_APPDATA".into(), "/Volumes/Data/vscode".into()));
        assert_eq!(store(&moved), PathBuf::from("/Volumes/Data/vscode/Cursor/User/globalStorage/state.vscdb"));
        assert_eq!(Cursor.env(), &["VSCODE_APPDATA"]);
        assert_eq!(Cursor.min_gap_s(), 300);
    }

    #[test]
    fn nothing_there_is_not_found() {
        let r = run(&[], Reach::Online, NOW, &[]);
        assert_eq!(r.outcome, Err(Trouble::NotFound));
        assert!(r.asked.is_empty());
        assert_eq!(r.pieces, 1);
    }

    #[test]
    fn no_token_is_signed_out() {
        let r = run(&files(SIGNED_OUT, None), Reach::Online, NOW, &[]);
        assert_eq!(r.outcome, Err(Trouble::SignedOut));
        assert!(r.asked.is_empty());
        // A store nothing was written to yet.
        assert_eq!(run(&files(b"", None), Reach::Online, NOW, &[]).outcome, Err(Trouble::SignedOut));
        let r = run(&files(b"{\"not\": \"sqlite\"}", None), Reach::Online, NOW, &[]);
        assert!(matches!(r.outcome, Err(Trouble::Unreadable(_))), "{:?}", r.outcome);
    }

    #[test]
    fn reads_keys_through_interior_and_overflow_pages() {
        let keys = ["cursorAuth/accessToken", "cursorAuth/cachedEmail", "cursorAuth/stripeMembershipType", "cursorAuth/notThere", "workbench.view.093"];
        let (got, asked) = values(&files(STATE, None), &keys);
        let got = got.unwrap();
        let token = got[0].as_deref().unwrap();
        // The renewed token (1048 bytes over two overflow pages of 512-byte
        // pages), not the older one left in freed pages.
        assert_eq!(token.len(), 1048);
        assert_eq!(exp_of(token), EXP);
        assert_eq!(got[1].as_deref(), Some("test@example.com"));
        assert_eq!(got[2].as_deref(), Some("pro"));
        assert_eq!(got[3], None);
        assert_eq!(got[4].as_deref(), Some(r#"{"i": 93, "open": false, "width": 293}"#));
        // A few small pieces, never the whole file.
        assert!(asked.len() <= 16, "{} pieces", asked.len());
        assert!(asked.iter().all(|p| p.len <= 4096));
    }

    #[test]
    fn reads_the_newest_values_from_the_wal() {
        let (got, asked) = values(&files(WAL_DB, Some((WAL_LOG, WAL_SHM))), &KEYS);
        let got = got.unwrap();
        assert_eq!(exp_of(got[0].as_deref().unwrap()), EXP);
        assert_eq!(got[2].as_deref(), Some("pro"));
        assert_eq!(got[1].as_deref(), Some("test@example.com"));
        // Never the index's lock bytes (120..128), which Windows holds.
        let shm = Part::Shm.path(&store(&Dirs::sample(Os::Windows)));
        for p in asked.iter().filter(|p| p.path == shm) {
            assert!(p.offset + p.len as u64 <= 120 || p.offset >= 128, "{p:?}");
        }
        // Cursor closed cleanly: the WAL and its index are gone and the main
        // file has what was last checkpointed.
        let got = values(&files(WAL_DB, None), &KEYS).0.unwrap();
        assert_eq!(exp_of(got[0].as_deref().unwrap()), 1_900_000_000);
        assert_eq!(got[2].as_deref(), Some("free_trial"));
    }

    #[test]
    fn a_wal_caught_mid_write_is_not_believed() {
        // The index header's two copies differ: SQLite was writing it.
        let mut shm = WAL_SHM.to_vec();
        shm[16] ^= 1;
        assert_eq!(values(&files(WAL_DB, Some((WAL_LOG, &shm[..]))), &KEYS).0, Err(Halt::Bad("Cursor's sign-in file changed while being read")));
        // Both copies changed alike, but the checksum doesn't match.
        shm[48 + 16] ^= 1;
        assert!(matches!(values(&files(WAL_DB, Some((WAL_LOG, &shm[..]))), &KEYS).0, Err(Halt::Bad(_))));
        // A frame from a newer WAL (its salts differ from the index's).
        let mut log = WAL_LOG.to_vec();
        for frame in 0..(log.len() - 32) / 536 {
            log[32 + frame * 536 + 8] ^= 0xff;
        }
        assert_eq!(values(&files(WAL_DB, Some((&log[..], WAL_SHM))), &KEYS).0, Err(Halt::Bad("Cursor's sign-in file changed while being read")));
        // The WAL is gone but its index stayed: not believed either.
        let gone = vec![files(WAL_DB, None).remove(0), (Part::Shm.path(&store(&Dirs::sample(Os::Windows))), WAL_SHM.to_vec())];
        assert!(matches!(values(&gone, &KEYS).0, Err(Halt::Bad(_))));
    }

    #[test]
    fn a_utf16_database_is_refused() {
        // Text encoding 2 (UTF-16LE): never made by VS Code or Cursor.
        let mut db = SIGNED_OUT.to_vec();
        db[59] = 2;
        assert!(matches!(values(&files(&db, None), &KEYS).0, Err(Halt::Bad(_))));
    }

    #[test]
    fn blobs_read_as_utf8_or_utf16() {
        assert_eq!(blob_text(b"eyJ.a.b").as_deref(), Some("eyJ.a.b"));
        // ASCII in UTF-16LE is also valid UTF-8 with NULs: read as UTF-16.
        assert_eq!(blob_text(b"e\0y\0J\0.\0").as_deref(), Some("eyJ."));
        assert_eq!(blob_text(&[0xff, 0xfe, 0xfd]), None);
    }

    #[test]
    fn a_damaged_store_never_panics() {
        // Every 61st byte flipped in turn: each walk ends, reading or refusing.
        for at in (0..STATE.len()).step_by(61) {
            let mut db = STATE.to_vec();
            db[at] ^= 0x5a;
            let _ = values(&files(&db, None), &KEYS);
        }
        // An interior page that points at itself ends at the visit limit.
        let mut db = STATE.to_vec();
        db[512 + 8..512 + 12].copy_from_slice(&2u32.to_be_bytes());
        let ptr = u16::from_be_bytes([db[512 + 12], db[512 + 13]]) as usize;
        db[512 + ptr..512 + ptr + 4].copy_from_slice(&2u32.to_be_bytes());
        let (got, asked) = values(&files(&db, None), &KEYS);
        assert!(matches!(got, Err(Halt::Bad(_))), "{got:?}");
        assert!(asked.len() < 64);
    }

    #[test]
    fn full_success() {
        let r = run(&files(STATE, None), Reach::Online, NOW, &[ok(SUMMARY), ok(NO_REQUESTS)]);
        let snap = r.outcome.clone().unwrap();
        let token = values(&files(STATE, None), &KEYS).0.unwrap()[0].clone().unwrap();
        assert_eq!(r.asked.len(), 2);
        let cookie = format!("WorkosCursorSessionToken=user_test%3A%3A{token}");
        assert_eq!(r.asked[0], Request::get("https://cursor.com/api/usage-summary").header("Accept", "application/json").header("Cookie", cookie.clone()));
        assert_eq!(r.asked[1], Request::get("https://cursor.com/api/usage?user=user_test").header("Accept", "application/json").header("Cookie", cookie));
        assert!(r.asked.iter().all(|q| !q.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("user-agent"))));
        assert_eq!(snap.provider, ProviderId::Cursor);
        assert_eq!(snap.plan.as_deref(), Some("Pro"));
        assert_eq!(snap.account.as_deref(), Some("t•••@example.com"));
        assert_eq!(snap.source, Source::Online);
        assert_eq!(snap.as_of, NOW);
        let resets = crate::parse_time("2026-11-18T10:27:04Z");
        let labels: Vec<_> = snap.windows.iter().map(|w| (w.label.as_str(), w.kind, w.resets_at, w.length_s)).collect();
        assert_eq!(labels, vec![
            ("Monthly", WindowKind::Monthly, resets, Some(31 * 86_400)),
            ("Monthly · Auto", WindowKind::Monthly, resets, Some(31 * 86_400)),
            ("Monthly · API", WindowKind::Monthly, resets, Some(31 * 86_400)),
        ]);
        let used: Vec<f32> = snap.windows.iter().map(|w| w.used).collect();
        assert!((used[0] - 0.194).abs() < 1e-6 && (used[1] - 0.125).abs() < 1e-6 && (used[2] - 0.31).abs() < 1e-6, "{used:?}");
        assert_eq!(snap.spend, vec![Spend { label: "Extra this month".into(), money: Money::usd(4.5), cap: Some(Money::usd(10.0)) }]);
        assert_eq!((snap.balance.clone(), snap.tokens_today, snap.days.len()), (None, None, 0));
        // The token never reaches the snapshot.
        assert!(!format!("{snap:?}").contains(&token[..40]));
    }

    #[test]
    fn a_request_based_plan_counts_requests() {
        let snap = run(&files(STATE, None), Reach::Online, NOW, &[ok(SUMMARY), ok(REQUESTS)]).outcome.unwrap();
        assert_eq!(snap.windows.len(), 1);
        let w = &snap.windows[0];
        assert_eq!((w.label.as_str(), w.kind, w.count), ("Requests", WindowKind::Monthly, Some((130, 500))));
        assert!((w.used - 0.26).abs() < 1e-6);
        // The request count failing hides nothing.
        let snap = run(&files(STATE, None), Reach::Online, NOW, &[ok(SUMMARY), status(404, None)]).outcome.unwrap();
        assert_eq!(snap.windows.len(), 3);
    }

    #[test]
    fn team_and_enterprise_fallbacks() {
        // No plan block: a member's own cap, then the team's on-demand budget.
        let summary = r#"{"membershipType":"enterprise","individualUsage":{"overall":{"enabled":true,"used":7384,"limit":10000},"onDemand":{"enabled":true,"used":250,"limit":null}},"teamUsage":{"onDemand":{"enabled":true,"used":2000,"limit":50000},"pooled":{"used":1,"limit":2}}}"#;
        let snap = run(&files(STATE, None), Reach::Online, NOW, &[ok(summary), ok(NO_REQUESTS)]).outcome.unwrap();
        assert_eq!(snap.plan.as_deref(), Some("Enterprise"));
        assert_eq!(snap.windows.len(), 1);
        assert_eq!((snap.windows[0].label.as_str(), snap.windows[0].resets_at), ("Monthly", None));
        assert!((snap.windows[0].used - 0.7384).abs() < 1e-6);
        assert_eq!(snap.spend, vec![Spend { label: "Team extra this month".into(), money: Money::usd(20.0), cap: Some(Money::usd(500.0)) }]);
        // Nothing to go on: no window, no spend (unknown is not zero), and
        // the plan from the store when the answer has none.
        let snap = run(&files(STATE, None), Reach::Online, NOW, &[ok(r#"{"individualUsage":{"onDemand":{"enabled":false,"used":0,"limit":null}}}"#), ok("{}")]).outcome.unwrap();
        assert!(snap.windows.is_empty() && snap.spend.is_empty());
        assert_eq!(snap.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn plan_names() {
        let names: Vec<String> = ["pro", "pro_plus", "free_trial", "ultra", "hobby", "enterprise", "team", "express", "mystery_tier"].iter().map(|k| plan_name(k)).collect();
        assert_eq!(names, ["Pro", "Pro+", "Pro trial", "Ultra", "Hobby", "Enterprise", "Team", "Start", "Mystery tier"]);
    }

    #[test]
    fn a_local_refresh_reads_the_store_and_never_asks() {
        let r = run(&files(STATE, None), Reach::Local, NOW, &[]);
        assert_eq!(r.outcome, Err(Trouble::NeedsOnline));
        assert!(r.asked.is_empty());
        assert_eq!(run(&files(SIGNED_OUT, None), Reach::Local, NOW, &[]).outcome, Err(Trouble::SignedOut));
        assert_eq!(run(&[], Reach::Local, NOW, &[]).outcome, Err(Trouble::NotFound));
        assert_eq!(run(&files(STATE, None), Reach::Local, EXP, &[]).outcome, Err(Trouble::Expired));
    }

    #[test]
    fn an_expired_sign_in_is_never_sent() {
        for now in [EXP - 60, EXP, EXP + 86_400] {
            let r = run(&files(STATE, None), Reach::Online, now, &[]);
            assert_eq!(r.outcome, Err(Trouble::Expired));
            assert!(r.asked.is_empty());
        }
        assert!(run(&files(STATE, None), Reach::Online, EXP - 61, &[ok(SUMMARY), ok("{}")]).outcome.is_ok());
    }

    #[test]
    fn tokens_that_dont_read() {
        let claims = |c: &str| format!("eyJhbGciOiJub25lIn0.{c}.sig");
        assert!(Sign::read(&claims("eyJleHAiOjIwMDAwMDAwMDB9")).is_none(), "no sub");
        assert!(Sign::read(&claims("eyJzdWIiOiJhdXRoMHx1c2VyX3Rlc3QifQ")).is_none(), "no exp");
        assert!(Sign::read(&claims("eyJzdWIiOiJhdXRoMHx1cyBlciIsImV4cCI6MjAwMDAwMDAwMH0")).is_none(), "a space in the user id");
        assert!(Sign::read("not a token").is_none());
        assert!(Sign::read("abc.def;Path=/.ghi").is_none());
        let s = Sign::read(&claims("eyJzdWIiOiJhdXRoMHx1c2VyX3Rlc3QiLCJleHAiOjIwMDAwMDAwMDAsImVtYWlsIjoiand0QGV4YW1wbGUuY29tIn0")).unwrap();
        assert_eq!((s.user.as_str(), s.exp, s.email.as_deref()), ("user_test", EXP, Some("jwt@example.com")));
    }

    #[test]
    fn refused_busy_offline() {
        let at = |answer| run(&files(STATE, None), Reach::Online, NOW, &[answer]).outcome;
        assert_eq!(at(status(401, None)), Err(Trouble::Refused));
        assert_eq!(at(status(403, None)), Err(Trouble::Refused));
        assert_eq!(at(status(429, Some(30))), Err(Trouble::Busy { retry_after_s: Some(30) }));
        assert_eq!(at(Err(NetFail::Timeout)), Err(Trouble::Offline));
        assert_eq!(at(status(502, None)), Err(Trouble::Offline));
    }

    #[test]
    fn an_answer_that_doesnt_read() {
        let at = |body: &str| run(&files(STATE, None), Reach::Online, NOW, &[ok(body)]).outcome;
        assert_eq!(at("<html>Just a moment...</html>"), Err(Trouble::Unreadable("not JSON".into())));
        assert!(matches!(at("[1, 2]"), Err(Trouble::Unreadable(_))));
        assert!(matches!(at(r#"{"error":"something else"}"#), Err(Trouble::Unreadable(_))));
    }

    #[test]
    fn the_contract_reads_the_store_a_piece_at_a_time() {
        // Through wants/next as the worker drives them: the same pieces and
        // outcomes as the lane's own loop.
        let dirs = Dirs::sample(Os::Windows);
        let fs = files(STATE, None);
        let piece = |w: &Want| match w {
            Want::Range { path, offset, len } => read(&fs, &Piece { path: path.clone(), offset: *offset, len: *len }),
            other => panic!("Cursor wanted {other:?}"),
        };
        let wants = Cursor.wants(&dirs);
        assert_eq!(wants, Cursor::first(&dirs).into_iter().map(Piece::want).collect::<Vec<_>>());
        let got: Vec<Got> = wants.iter().map(piece).collect();
        let local = Ctx { dirs: &dirs, got: &got, now: NOW, reach: Reach::Local };
        let (asked, out) = crate::drive_reads(&Cursor, &local, piece, |_| unreachable!("fetched on a local refresh"));
        assert!(asked.is_empty());
        assert_eq!(out, run(&fs, Reach::Local, NOW, &[]).outcome);
        let online = Ctx { reach: Reach::Online, ..local };
        let (asked, out) = crate::drive_reads(&Cursor, &online, piece, |_| ok(SUMMARY));
        let lane = run(&fs, Reach::Online, NOW, &[ok(SUMMARY), ok(SUMMARY)]);
        assert_eq!((asked.len(), out.is_ok()), (lane.asked.len(), lane.outcome.is_ok()));
    }
}
