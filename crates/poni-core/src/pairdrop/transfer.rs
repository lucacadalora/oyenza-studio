//! Files over the data channel: the messages, files coming in from a phone,
//! and Shelf files going out to it (`docs/paired-drop.md`, "The data channel").
//!
//! A send is one list of files followed by their bytes back to back; the
//! sizes the phone announced split the stream, so a file can't grow past
//! what was announced. Files are staged and moved into the inbox only when
//! every byte of every file has arrived, like today's phone drop.

use crate::phonedrop::names::{safe_name, unique_path};
use serde::Deserialize;
use serde_json::json;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Most files in one send.
pub const MAX_FILES: usize = 500;
/// Largest send, all files together.
pub const MAX_TOTAL: u64 = 64 * 1024 * 1024 * 1024;
/// Bytes per binary frame the PC sends.
pub const FRAME: usize = 16 * 1024;
/// Staging folders in the inbox: `<prefix><n>`. Not the same prefix as the
/// QR phone drop's, which clears its own leftovers whenever it opens.
pub const STAGING_PREFIX: &str = ".oyen-poni-paired-";

/// What a phone says on the data channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromPhone {
    Hello { name: String },
    Send(Vec<(String, u64)>),
    Get { id: u64 },
    Bye,
    /// Anything we don't know (newer phone page): ignored.
    Other,
}

#[derive(Deserialize)]
struct Raw {
    t: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    files: Vec<RawFile>,
    #[serde(default)]
    id: Option<u64>,
}

#[derive(Deserialize)]
struct RawFile {
    name: String,
    size: u64,
}

/// Read one text frame. `None` = not JSON at all.
pub fn parse(text: &[u8]) -> Option<FromPhone> {
    let r: Raw = serde_json::from_slice(text).ok()?;
    Some(match r.t.as_str() {
        "hello" => FromPhone::Hello { name: super::signal::clean_name(&r.name) },
        "send" => FromPhone::Send(r.files.into_iter().map(|f| (f.name, f.size)).collect()),
        "get" => match r.id {
            Some(id) => FromPhone::Get { id },
            None => FromPhone::Other,
        },
        "bye" => FromPhone::Bye,
        _ => FromPhone::Other,
    })
}

/// A Shelf file a phone may save: its id in this session, name and size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShelfFile {
    pub id: u64,
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
}

fn shelf_json(files: &[ShelfFile]) -> serde_json::Value {
    files.iter().map(|f| json!({ "id": f.id, "name": f.name, "size": f.size })).collect()
}

pub fn hello(pc_name: &str, shelf: &[ShelfFile]) -> String {
    json!({ "t": "hello", "v": 1, "name": pc_name, "auto": true, "shelf": shelf_json(shelf) }).to_string()
}

pub fn shelf(files: &[ShelfFile]) -> String {
    json!({ "t": "shelf", "files": shelf_json(files) }).to_string()
}

pub fn ok() -> String {
    json!({ "t": "ok" }).to_string()
}

pub fn no(why: &str) -> String {
    json!({ "t": "no", "why": why }).to_string()
}

pub fn got(n: u64) -> String {
    json!({ "t": "got", "n": n }).to_string()
}

pub fn saved(files: usize) -> String {
    json!({ "t": "saved", "files": files }).to_string()
}

pub fn failed(why: &str) -> String {
    json!({ "t": "failed", "why": why }).to_string()
}

pub fn file_head(f: &ShelfFile) -> String {
    json!({ "t": "file", "id": f.id, "name": f.name, "size": f.size }).to_string()
}

pub fn nofile(id: u64) -> String {
    json!({ "t": "nofile", "id": id }).to_string()
}

/// Check a `send` list; the names come back made safe for Windows.
pub fn check_send(files: &[(String, u64)]) -> Result<Vec<(String, u64)>, &'static str> {
    if files.is_empty() || files.len() > MAX_FILES {
        return Err("too-many");
    }
    let total = files.iter().try_fold(0u64, |acc, f| acc.checked_add(f.1)).ok_or("too-large")?;
    if total > MAX_TOTAL {
        return Err("too-large");
    }
    Ok(files.iter().map(|(n, s)| (safe_name(n), *s)).collect())
}

/// How a send ended badly; `why` is the word the phone gets.
#[derive(Debug)]
pub struct Broken {
    pub why: &'static str,
    pub detail: String,
}

/// Files coming in: written to `<inbox>/<prefix><n>/<i>.part`, then moved.
pub struct Incoming {
    files: Vec<(String, u64)>,
    staging: PathBuf,
    inbox: PathBuf,
    index: usize,
    written: u64,
    out: Option<BufWriter<File>>,
    pub done: u64,
    pub total: u64,
}

impl Incoming {
    /// Start a send of `files` (already checked) into `inbox`.
    pub fn start(inbox: &Path, n: u64, files: Vec<(String, u64)>) -> Result<Incoming, Broken> {
        let staging = inbox.join(format!("{STAGING_PREFIX}{n}"));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(|e| Broken { why: "disk", detail: e.to_string() })?;
        let total = files.iter().map(|f| f.1).sum();
        let mut me = Incoming { files, staging, inbox: inbox.to_path_buf(), index: 0, written: 0, out: None, done: 0, total };
        me.open_current().map_err(|e| me.abort("disk", e))?;
        Ok(me)
    }

    pub fn files(&self) -> usize {
        self.files.len()
    }

    /// The file being written (0-based; = files() once all are written).
    pub fn current(&self) -> usize {
        self.index
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|f| f.0.as_str())
    }

    /// Open file `index` (and step over empty files, which are complete at once).
    fn open_current(&mut self) -> io::Result<()> {
        while self.index < self.files.len() {
            let path = self.staging.join(format!("{}.part", self.index));
            let f = File::create(&path)?;
            if self.files[self.index].1 > 0 {
                self.out = Some(BufWriter::with_capacity(256 * 1024, f));
                self.written = 0;
                return Ok(());
            }
            self.index += 1;
        }
        self.out = None;
        Ok(())
    }

    /// Take the next bytes. `Ok(Some(paths))` when the last byte arrived and
    /// everything was moved into the inbox.
    pub fn take(&mut self, mut data: &[u8]) -> Result<Option<Vec<PathBuf>>, Broken> {
        while !data.is_empty() {
            if self.index >= self.files.len() {
                return Err(self.abort("size", io::Error::new(io::ErrorKind::InvalidData, "more bytes than the files announced")));
            }
            let left = self.files[self.index].1 - self.written;
            let n = (left as usize).min(data.len());
            let r = match self.out.as_mut() {
                Some(out) => out.write_all(&data[..n]),
                None => Err(io::Error::other("no file open")),
            };
            if let Err(e) = r {
                return Err(self.abort("disk", e));
            }
            self.written += n as u64;
            self.done += n as u64;
            data = &data[n..];
            if self.written == self.files[self.index].1 {
                if let Some(mut out) = self.out.take() {
                    if let Err(e) = out.flush().and_then(|_| out.get_ref().sync_all()) {
                        return Err(self.abort("disk", e));
                    }
                }
                self.index += 1;
                if let Err(e) = self.open_current() {
                    return Err(self.abort("disk", e));
                }
            }
        }
        if self.index >= self.files.len() {
            return match self.commit() {
                Ok(paths) => Ok(Some(paths)),
                Err(e) => Err(self.abort("disk", e)),
            };
        }
        Ok(None)
    }

    /// A send of only empty files is complete before any byte arrives.
    pub fn finished_without_bytes(&mut self) -> Result<Option<Vec<PathBuf>>, Broken> {
        if self.index >= self.files.len() {
            self.take(&[])
        } else {
            Ok(None)
        }
    }

    /// Move every staged file into the inbox (numbered when a name is taken);
    /// if one move fails, the moved ones are taken back out.
    fn commit(&mut self) -> io::Result<Vec<PathBuf>> {
        let mut placed: Vec<PathBuf> = Vec::with_capacity(self.files.len());
        for (i, (name, _)) in self.files.iter().enumerate() {
            let dest = unique_path(&self.inbox, name, &placed);
            if let Err(e) = fs::rename(self.staging.join(format!("{i}.part")), &dest) {
                for p in &placed {
                    let _ = fs::remove_file(p);
                }
                return Err(e);
            }
            placed.push(dest);
        }
        let _ = fs::remove_dir_all(&self.staging);
        Ok(placed)
    }

    /// Give up: nothing from this send is kept.
    pub fn abort(&mut self, why: &'static str, e: io::Error) -> Broken {
        self.out = None;
        let _ = fs::remove_dir_all(&self.staging);
        Broken { why, detail: e.to_string() }
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        // A send that never finished leaves nothing behind.
        self.out = None;
        let _ = fs::remove_dir_all(&self.staging);
    }
}

/// Leftover staging folders from a send cut off when the app quit.
pub fn clear_stale_staging(inbox: &Path) {
    let Ok(entries) = fs::read_dir(inbox) else { return };
    for e in entries.flatten() {
        let is_staging = e.file_name().to_str().is_some_and(|n| n.starts_with(STAGING_PREFIX));
        if is_staging && e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// A Shelf file on its way to the phone.
pub struct Outgoing {
    file: File,
    pub left: u64,
}

impl Outgoing {
    pub fn open(f: &ShelfFile) -> io::Result<Outgoing> {
        let file = File::open(&f.path)?;
        let left = file.metadata()?.len();
        if left != f.size {
            return Err(io::Error::other("the file changed since the Shelf list was sent"));
        }
        Ok(Outgoing { file, left })
    }

    /// The next frame (at most [`FRAME`] bytes); empty when done.
    pub fn next_frame(&mut self) -> io::Result<Vec<u8>> {
        let want = (self.left as usize).min(FRAME);
        let mut buf = vec![0u8; want];
        self.file.read_exact(&mut buf)?;
        self.left -= want as u64;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("poni-pairdrop-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_phone_messages() {
        assert_eq!(parse(br#"{"t":"hello","v":1,"name":"Pixel\n10"}"#), Some(FromPhone::Hello { name: "Pixel 10".into() }));
        assert_eq!(parse(br#"{"t":"send","files":[{"name":"a.jpg","size":3}]}"#), Some(FromPhone::Send(vec![("a.jpg".into(), 3)])));
        assert_eq!(parse(br#"{"t":"get","id":7}"#), Some(FromPhone::Get { id: 7 }));
        assert_eq!(parse(br#"{"t":"get"}"#), Some(FromPhone::Other));
        assert_eq!(parse(br#"{"t":"bye"}"#), Some(FromPhone::Bye));
        assert_eq!(parse(br#"{"t":"later-thing","x":1}"#), Some(FromPhone::Other));
        assert!(parse(b"not json").is_none());
    }

    #[test]
    fn checks_send_lists() {
        assert_eq!(check_send(&[]), Err("too-many"));
        assert_eq!(check_send(&vec![("a".into(), 1); MAX_FILES + 1]), Err("too-many"));
        assert_eq!(check_send(&[("a".into(), MAX_TOTAL), ("b".into(), 1)]), Err("too-large"));
        assert_eq!(check_send(&[("a".into(), u64::MAX), ("b".into(), 2)]), Err("too-large"));
        assert_eq!(check_send(&[("../evil:name?.txt".into(), 2)]).unwrap(), vec![("evil_name_.txt".to_string(), 2)]);
    }

    #[test]
    fn splits_the_stream_by_size_and_commits() {
        let d = dir("split");
        let files = vec![("one.txt".to_string(), 5), ("empty.txt".to_string(), 0), ("two.txt".to_string(), 7)];
        let mut inc = Incoming::start(&d, 1, files).unwrap();
        assert_eq!(inc.take(b"hel").unwrap(), None);
        // One frame that ends the first file, skips the empty one and starts the third.
        assert_eq!(inc.take(b"loworl").unwrap(), None);
        assert_eq!(inc.current(), 2);
        let paths = inc.take(b"d!!").unwrap().unwrap();
        assert_eq!(paths.len(), 3);
        assert_eq!(fs::read(&paths[0]).unwrap(), b"hello");
        assert_eq!(fs::read(&paths[1]).unwrap(), b"");
        assert_eq!(fs::read(&paths[2]).unwrap(), b"world!!");
        assert_eq!(inc.done, 12);
        assert!(!d.join(format!("{STAGING_PREFIX}1")).exists());
        // The same names again get numbered.
        let mut again = Incoming::start(&d, 2, vec![("one.txt".into(), 1)]).unwrap();
        let p = again.take(b"x").unwrap().unwrap();
        assert_eq!(p[0].file_name().unwrap(), "one (1).txt");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn too_many_bytes_keep_nothing() {
        let d = dir("over");
        let mut inc = Incoming::start(&d, 3, vec![("a.bin".into(), 2)]).unwrap();
        let err = inc.take(b"abc").unwrap_err();
        assert_eq!(err.why, "size");
        assert!(!d.join(format!("{STAGING_PREFIX}3")).exists());
        assert_eq!(fs::read_dir(&d).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_sends_finish_at_once_and_cut_sends_vanish() {
        let d = dir("empty");
        let mut inc = Incoming::start(&d, 4, vec![("z".into(), 0)]).unwrap();
        let p = inc.finished_without_bytes().unwrap().unwrap();
        assert_eq!(fs::read(&p[0]).unwrap(), b"");
        let mut cut = Incoming::start(&d, 5, vec![("half.bin".into(), 10)]).unwrap();
        assert_eq!(cut.take(b"12345").unwrap(), None);
        drop(cut);
        assert!(!d.join(format!("{STAGING_PREFIX}5")).exists());
        assert!(!d.join("half.bin").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn sends_shelf_files_in_frames() {
        let d = dir("out");
        let path = d.join("big.bin");
        let data: Vec<u8> = (0..(FRAME * 2 + 100)).map(|i| (i % 251) as u8).collect();
        fs::write(&path, &data).unwrap();
        let f = ShelfFile { id: 1, path, name: "big.bin".into(), size: data.len() as u64 };
        let mut out = Outgoing::open(&f).unwrap();
        let mut back = Vec::new();
        while out.left > 0 {
            back.extend(out.next_frame().unwrap());
        }
        assert_eq!(back, data);
        let stale = ShelfFile { size: 5, ..f };
        assert!(Outgoing::open(&stale).is_err());
        let _ = fs::remove_dir_all(&d);
    }
}
