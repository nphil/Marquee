//! The profile pictures of the "who's watching" screen, kept DECODED: raw RGBA at the size the
//! picker draws them, so a launch reads one small file and uploads it straight to a texture
//! instead of waiting on the directory scan, a file read, a JPEG decode and a worker hand-off.
//!
//! One file (`tiles.img`) holds every tile and is rewritten whole through a temporary and a
//! rename: a reader sees the old set or the new one, and an interrupted write leaves no torn
//! file. The name ends in `.img`, so [`Cache::clear`] already removes it at sign-out; the
//! generation check under [`Gate`] keeps a write that was in flight from bringing it back.
//!
//! A tile is named by the avatar's plex.tv address and the transcode options that shaped it,
//! never by its `c=` version stamp or any credential ([`identify`]). The stamp is stored beside
//! the pixels instead, so a roster that carries a newer stamp still shows the stored picture at
//! once and marks it due for a check ([`Tile::due`]).
//!
//! The pixels stay uncompressed on purpose: decoding is the cost this removes. Three profiles at
//! 300x300 are about a megabyte.

use super::{
    byte_cap, cache, free_bytes, hex_digest, is_avatar, percent_decode, probe_dir, stale,
    transcode_request, without_avatar_stamp, Cache,
};
use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The most tiles one file holds, more than any Plex Home has profiles. A tile past it is not
/// stored: its picture loads the ordinary way (a disk-cache hit or a fetch, then a decode).
pub const MAX_TILES: usize = 16;
/// The largest width or height a tile may have. The picker asks the transcoder for 300x300.
const LARGEST_SIDE: usize = 512;
/// The longest version stamp kept; a Unix time is ten digits.
const LONGEST_STAMP: usize = 64;
const MAGIC: &[u8; 8] = b"PLXAVT01";
/// Magic, tile count (u32), reserved (u32).
const FILE_HEADER: usize = 16;
/// Name (64 hex digits), fetched (u64), width, height, stamp length, reserved (four u16).
const ENTRY_HEADER: usize = 80;
const FILE_NAME: &str = "tiles.img";
const DOMAIN: &str = "plx-avatar-tile-v1";
/// What the largest well-formed file occupies; anything bigger is not one of ours.
const LARGEST_FILE: u64 =
    (FILE_HEADER + MAX_TILES * (ENTRY_HEADER + LONGEST_STAMP + LARGEST_SIDE * LARGEST_SIDE * 4)) as u64;

/// What names a stored picture and what tells its versions apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// 64 lowercase hex digits: the picture's source and shape, never its version or a credential.
    pub name: String,
    /// The avatar's own `c=` version, empty when the address carries none.
    pub stamp: String,
}

/// The identity of the picture a built photo-transcoder request asks for, or `None` when it is
/// not a profile picture: only an avatar address on plex.tv qualifies, so headshots and posters
/// never occupy a tile. The server is not part of it: plex.tv serves the picture whichever
/// server resizes it. Every transcode option is, so a different size is a different tile.
pub fn identify(built_key: &str) -> Option<Identity> {
    let (mut params, source) = transcode_request(built_key)?;
    let url = std::mem::take(&mut params[source].1);
    if !is_avatar(&url) {
        return None;
    }
    let stamp = avatar_stamp(&url);
    if stamp.len() > LONGEST_STAMP {
        return None;
    }
    params[source].1 = without_avatar_stamp(&url);
    params.sort();
    let mut identity = Vec::new();
    for field in
        std::iter::once(DOMAIN).chain(params.iter().flat_map(|(k, v)| [k.as_str(), v.as_str()]))
    {
        identity.extend_from_slice(&(field.len() as u64).to_le_bytes());
        identity.extend_from_slice(field.as_bytes());
    }
    Some(Identity {
        name: hex_digest(&identity),
        stamp,
    })
}

fn avatar_stamp(source: &str) -> String {
    let Some((_, query)) = source.split_once('?') else {
        return String::new();
    };
    query
        .split('&')
        .find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (percent_decode(k) == "c").then(|| percent_decode(v))
        })
        .unwrap_or_default()
}

/// One decoded picture, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub name: String,
    pub stamp: String,
    /// Unix seconds the picture was last confirmed against the server; 0 is never confirmed, so
    /// the tile is always due.
    pub fetched: u64,
    pub w: u16,
    pub h: u16,
    pub rgba: Box<[u8]>,
}

fn sized(w: usize, h: usize) -> bool {
    (1..=LARGEST_SIDE).contains(&w) && (1..=LARGEST_SIDE).contains(&h)
}

fn valid_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Tile {
    /// `None` when the pixels are not exactly `w * h` RGBA texels of a size a tile may have.
    pub fn new(id: Identity, fetched: u64, w: usize, h: usize, rgba: &[u8]) -> Option<Self> {
        if !sized(w, h) || rgba.len() != w * h * 4 {
            return None;
        }
        Some(Self {
            name: id.name,
            stamp: id.stamp,
            fetched,
            w: w as u16,
            h: h as u16,
            rgba: rgba.into(),
        })
    }

    fn valid(&self) -> bool {
        valid_name(&self.name)
            && self.stamp.len() <= LONGEST_STAMP
            && sized(self.w.into(), self.h.into())
            && self.rgba.len() == usize::from(self.w) * usize::from(self.h) * 4
    }

    /// Whether the picture is due a check against the server at Unix time `now`: the roster's
    /// version stamp is not the one it was stored under, it was never confirmed, or it has gone
    /// unconfirmed for [`super::REFRESH_AFTER`] (a clock that reads before the fetch counts as
    /// unconfirmed too).
    pub fn due(&self, stamp: &str, now: u64) -> bool {
        self.stamp != stamp
            || self.fetched == 0
            || stale(self.fetched, UNIX_EPOCH + Duration::from_secs(now))
    }
}

/// Unix seconds now: the clock `fetched` is kept in.
pub fn now() -> u64 {
    super::unix_seconds(SystemTime::now())
}

/// Serialises tile writes with each other and with [`Cache::clear`]. The directory it writes to is
/// chosen on the first write and forgotten by a clear.
#[derive(Default)]
pub(super) struct Gate {
    root: Option<PathBuf>,
}

/// Every stored tile and the generation they were read under. The generation is read BEFORE the
/// file, so a sign-out that lands in between leaves tiles that no longer match
/// [`super::generation`] and the caller drops them.
pub fn load() -> (u64, Vec<Tile>) {
    let c = cache();
    let generation = c.generation.load(Ordering::Acquire);
    (generation, c.load_tiles())
}

/// Replaces the stored set with `tiles`, unless a sign-out has bumped the generation since
/// `generation` was read. An empty set removes the file. `false` when nothing was written: a
/// stale generation, too many or malformed tiles, no writable directory, too little free space.
pub fn save(generation: u64, tiles: &[&Tile]) -> bool {
    cache().save_tiles(generation, tiles)
}

impl Cache {
    pub(super) fn load_tiles(&self) -> Vec<Tile> {
        self.candidates
            .iter()
            .find_map(|root| read_file(&root.join(FILE_NAME)))
            .unwrap_or_default()
    }

    pub(super) fn save_tiles(&self, generation: u64, tiles: &[&Tile]) -> bool {
        if tiles.len() > MAX_TILES || !tiles.iter().all(|t| t.valid()) {
            return false;
        }
        let mut gate = self.tiles.lock().unwrap_or_else(|e| e.into_inner());
        if self.generation.load(Ordering::Acquire) != generation {
            return false;
        }
        let root = match &gate.root {
            Some(root) => root.clone(),
            None => {
                let found = self.candidates.iter().find(|p| {
                    probe_dir(p, plx_base::paths::image_cache_dir_is_private(p))
                });
                let Some(root) = found else {
                    return false;
                };
                gate.root = Some(root.clone());
                root.clone()
            }
        };
        let dst = root.join(FILE_NAME);
        if tiles.is_empty() {
            return match fs::remove_file(&dst) {
                Ok(()) => true,
                Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            };
        }
        let body = encode(tiles);
        let len = body.len() as u64;
        let held = fs::metadata(&dst).map_or(0, |m| m.len());
        // The same rule the disk cache keeps: never take more than half of what the filesystem
        // could spare, and none of it on a nearly full one.
        if byte_cap(len, free_bytes(&root), held).is_none_or(|cap| len > cap) {
            return false;
        }
        let tmp = root.join(format!("{FILE_NAME}.tmp"));
        let written = fs::write(&tmp, &body).is_ok() && fs::rename(&tmp, &dst).is_ok();
        if !written {
            let _ = fs::remove_file(&tmp);
        }
        written
    }
}

fn encode(tiles: &[&Tile]) -> Vec<u8> {
    let payload: usize = tiles
        .iter()
        .map(|t| ENTRY_HEADER + t.stamp.len() + t.rgba.len())
        .sum();
    let mut out = Vec::with_capacity(FILE_HEADER + payload);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(tiles.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for t in tiles {
        out.extend_from_slice(t.name.as_bytes());
        out.extend_from_slice(&t.fetched.to_le_bytes());
        out.extend_from_slice(&t.w.to_le_bytes());
        out.extend_from_slice(&t.h.to_le_bytes());
        out.extend_from_slice(&(t.stamp.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(t.stamp.as_bytes());
        out.extend_from_slice(&t.rgba);
    }
    out
}

/// The file at `path` if it is a whole, well-formed set; a file that is anything else is not read
/// further. The size is taken from the OPEN handle, so a file swapped in after the open cannot
/// make the read larger than what was checked.
fn read_file(path: &std::path::Path) -> Option<Vec<Tile>> {
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > LARGEST_FILE {
        return None;
    }
    let mut bytes = vec![0u8; meta.len() as usize];
    file.read_exact(&mut bytes).ok()?;
    parse(&bytes)
}

/// A cursor over the file's bytes: every read is bounds-checked and a short file is `None`.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn read(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.read(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.read(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.read(8)?.try_into().ok()?))
    }
}

fn parse(bytes: &[u8]) -> Option<Vec<Tile>> {
    let mut r = Reader(bytes);
    if r.read(MAGIC.len())? != MAGIC {
        return None;
    }
    let count = r.u32()? as usize;
    r.u32()?;
    if count > MAX_TILES {
        return None;
    }
    let mut tiles: Vec<Tile> = Vec::with_capacity(count);
    for _ in 0..count {
        let name = std::str::from_utf8(r.read(64)?).ok()?;
        let fetched = r.u64()?;
        let (w, h) = (r.u16()?, r.u16()?);
        let stamp_len = usize::from(r.u16()?);
        r.u16()?;
        if !valid_name(name) || !sized(w.into(), h.into()) || stamp_len > LONGEST_STAMP {
            return None;
        }
        let stamp = std::str::from_utf8(r.read(stamp_len)?).ok()?;
        let rgba = r.read(usize::from(w) * usize::from(h) * 4)?;
        if tiles.iter().any(|t| t.name == name) {
            return None;
        }
        tiles.push(Tile {
            name: name.to_owned(),
            stamp: stamp.to_owned(),
            fetched,
            w,
            h,
            rgba: rgba.into(),
        });
    }
    r.0.is_empty().then_some(tiles)
}
