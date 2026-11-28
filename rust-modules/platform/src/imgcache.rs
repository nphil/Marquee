//! Shared durable tier for all Plex image transcodes and for the images the app bakes from them
//! (a collection's fan, [`classify_baked`]). The poster workers own fetch/decode/bake;
//! this module owns canonical identities, bounded disk storage, and sign-out generations.
//!
//! The index is scanned once, lazily on a worker's first access or ahead of it by [`prewarm`].
//! Steady-state reads/writes use an in-memory ordered LRU, never a directory scan. Files contain
//! only a versioned timestamp header and compressed image bytes; names hash the server, source,
//! and transformation.
//! Fetch time drives refresh independently of recency, which is persisted at most hourly.
//! Limits cover committed files; one atomic-write temporary can additionally occupy MAX_FILE.
//!
//! A `tmdb-` picture expires from its fetch time. Older `image-` files don't record their source,
//! so they share that deadline until a request identifies and renames them. All are rebuildable
//! files in this cache; no offline downloads or user-supplied originals are stored here.
//! Newly cached `plex-` art is still evicted only by size/recency.
//! Expired entries are never served and are removed on startup and background maintenance.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const REFRESH_AFTER: Duration = Duration::from_secs(24 * 3600);
const TOUCH_AFTER: Duration = Duration::from_secs(3600);
const MAX_FILE: usize = 4 * 1024 * 1024;
const MAX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 8192;
const MAGIC: &[u8; 8] = b"PLXIMG02";
const HEADER_LEN: usize = 24;
const TMDB_PREFIX: &str = "tmdb-";
const PLEX_PREFIX: &str = "plex-";
const UNSCOPED_PREFIX: &str = "image-";
const SWEEP_AFTER: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiskKey {
    name: String,
    legacy: Option<String>,
    /// Old undifferentiated name, adopted on read while it is within its retention window.
    prior: Option<String>,
}

pub struct CachedImage {
    pub bytes: Vec<u8>,
    pub stale: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub hit: u64,
    pub miss: u64,
    pub write: u64,
    pub eviction: u64,
    pub entries: u64,
    pub bytes: u64,
}

/// Every transcode option participates, except the outer authentication token. Namespace is
/// the stable Plex machine identifier (or server origin when discovery has no identifier).
pub fn classify(server_namespace: &str, built_key: &str) -> Option<DiskKey> {
    if server_namespace.is_empty() {
        return None;
    }
    let (mut params, source_index) = transcode_request(built_key)?;
    let avatar = is_avatar(&params[source_index].1);
    let tmdb = is_tmdb_source(&params[source_index].1);
    if avatar {
        params[source_index].1 = without_avatar_stamp(&params[source_index].1);
    }
    // The previous avatar cache knew only size/minSize=1, and ignored every source query.
    // Migrate only that exact request shape, never a different format, crop, or version.
    let legacy = if avatar
        && !params[source_index].1.contains('?')
        && params.iter().all(|(k, v)| {
            matches!(k.as_str(), "url" | "width" | "height") || (k == "minSize" && v == "1")
        }) {
        let value = |name: &str| {
            params
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        Some(format!(
            "avatar-{}.img",
            hex_digest(
                format!(
                    "avatar\n{}\n{}x{}",
                    value("url"),
                    value("width"),
                    value("height")
                )
                .as_bytes()
            )
        ))
    } else {
        None
    };
    params.sort();
    let mut identity = Vec::new();
    for field in std::iter::once("plx-image-v2")
        .chain(std::iter::once(server_namespace))
        .chain(params.iter().flat_map(|(k, v)| [k.as_str(), v.as_str()]))
    {
        identity.extend_from_slice(&(field.len() as u64).to_le_bytes());
        identity.extend_from_slice(field.as_bytes());
    }
    let hash = hex_digest(&identity);
    Some(DiskKey {
        name: format!("{}{hash}.img", if tmdb { TMDB_PREFIX } else { PLEX_PREFIX }),
        legacy,
        prior: Some(format!("{UNSCOPED_PREFIX}{hash}.img")),
    })
}

/// The options of a photo-transcoder request with the outer authentication token dropped, and the
/// position of its one `url` (the source picture): present, not empty, and not a scrub-preview
/// frame. The front half of every identity derived from a built request.
fn transcode_request(built_key: &str) -> Option<(Vec<(String, String)>, usize)> {
    let (path, query) = built_key.split_once('?')?;
    if path != "/photo/:/transcode" {
        return None;
    }
    let params: Vec<(String, String)> = query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k), percent_decode(v))
        })
        .filter(|(k, _)| !k.eq_ignore_ascii_case("X-Plex-Token"))
        .collect();
    let mut sources = params
        .iter()
        .enumerate()
        .filter_map(|(i, (k, _))| (k == "url").then_some(i));
    let source = sources.next()?;
    if sources.next().is_some() || params[source].1.is_empty() || is_preview_frame(&params[source].1) {
        return None;
    }
    Some((params, source))
}

/// The durable key of an image the APP rendered (a collection fan), not one the server sent.
/// It shares the directory, LRU and byte bound with transcodes, but its identity is its own
/// versioned domain: the namespace (server machine identity plus the profile whose visibility
/// chose the members), what was baked, the source item, the server's stamp for that item, and
/// the output size. A new stamp is a new key, so an edited collection re-bakes and the old file
/// ages out of the LRU. No field is a credential; callers pass identities only.
pub fn classify_baked(
    namespace: &str,
    kind: &str,
    rating_key: &str,
    stamp: &str,
    w: u32,
    h: u32,
) -> Option<DiskKey> {
    if [namespace, kind, rating_key, stamp].iter().any(|f| f.is_empty()) || w == 0 || h == 0 {
        return None;
    }
    let dims = format!("{w}x{h}");
    let mut identity = Vec::new();
    for field in ["plx-baked-v1", namespace, kind, rating_key, stamp, &dims] {
        identity.extend_from_slice(&(field.len() as u64).to_le_bytes());
        identity.extend_from_slice(field.as_bytes());
    }
    let hash = hex_digest(&identity);
    Some(DiskKey {
        name: format!("{PLEX_PREFIX}{hash}.img"),
        legacy: None,
        prior: Some(format!("{UNSCOPED_PREFIX}{hash}.img")),
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    plx_base::sha256::sha256(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A still out of a part's BIF index (`/library/parts/{id}/indexes/{kind}/{ms}`), the scrub
/// preview's source. Ephemeral by nature — one per scrub position, rarely asked for twice — so it is
/// never given a disk identity: the LRU it would churn is the posters'.
fn is_preview_frame(source: &str) -> bool {
    let mut parts = source.split('?').next().unwrap_or(source).split('/');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(""), Some("library"), Some("parts"), Some(id), Some("indexes")) if !id.is_empty()
    )
}

fn is_avatar(source: &str) -> bool {
    let Some(rest) = source
        .strip_prefix("https://plex.tv/")
        .or_else(|| source.strip_prefix("http://plex.tv/"))
    else {
        return false;
    };
    let mut parts = rest.split('?').next().unwrap_or(rest).split('/');
    matches!((parts.next(), parts.next(), parts.next()), (Some("users"), Some(id), Some("avatar")) if !id.is_empty())
}

fn without_avatar_stamp(source: &str) -> String {
    let Some((path, query)) = source.split_once('?') else {
        return source.into();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| percent_decode(pair.split_once('=').map_or(*pair, |(k, _)| k)) != "c")
        .collect();
    if kept.is_empty() {
        path.into()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

/// RFC3986 decoding: literal `+` is not a form-encoded space.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(hi), Some(lo)) = (
                (b[i + 1] as char).to_digit(16),
                (b[i + 2] as char).to_digit(16),
            ) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Clone, Copy)]
struct Limits {
    bytes: u64,
    entries: usize,
}

struct Entry {
    size: u64,
    order: u64,
    touched: SystemTime,
    /// Original fetch time for TMDB and undifferentiated entries.
    fetched: Option<u64>,
}

#[derive(Default)]
struct Index {
    initialized: bool,
    root: Option<PathBuf>,
    entries: HashMap<String, Entry>,
    lru: BTreeSet<(u64, String)>,
    bytes: u64,
    sequence: u64,
    /// The byte ceiling for THIS root, set once it is chosen: [`Limits::bytes`] clamped to the
    /// space the filesystem can spare (see [`byte_cap`]). `None` before initialization.
    cap: Option<u64>,
    /// When the TMDB entries were last checked against the retention limit ([`Cache::sweep_expired`]).
    swept: Option<SystemTime>,
}

#[derive(Default)]
struct Counters {
    hit: AtomicU64,
    miss: AtomicU64,
    write: AtomicU64,
    eviction: AtomicU64,
    entries: AtomicU64,
    bytes: AtomicU64,
}

/// All filesystem access shares this gate, including reads and removes: an old worker cannot
/// read or delete a new account's entry after clear. Constructors do no filesystem work.
struct Cache {
    candidates: Vec<PathBuf>,
    limits: Limits,
    generation: AtomicU64,
    index: Mutex<Index>,
    counters: Counters,
    /// Serialises avatar-tile writes with each other and with [`Cache::clear`] (see [`tiles`]).
    tiles: Mutex<tiles::Gate>,
}

impl Cache {
    fn new(candidates: Vec<PathBuf>, limits: Limits) -> Self {
        Self {
            candidates,
            limits,
            generation: AtomicU64::new(0),
            index: Mutex::new(Index::default()),
            counters: Counters::default(),
            tiles: Mutex::default(),
        }
    }

    fn initialize(&self, index: &mut Index) {
        if index.initialized {
            return;
        }
        index.initialized = true;
        index.root = self
            .candidates
            .iter()
            .find(|p| probe_dir(p, plx_base::paths::image_cache_dir_is_private(p)))
            .cloned();
        let now = SystemTime::now();
        index.swept = Some(now);
        self.sweep_other_roots(index.root.as_deref(), unix_seconds(now));
        let Some(root) = index.root.clone() else {
            plx_base::eventlog::log("imgcache: no writable directory; disk cache unavailable");
            return;
        };
        let Ok(files) = fs::read_dir(&root) else {
            return;
        };
        let mut found = Vec::new();
        let now = unix_seconds(now);
        let mut dropped = 0u32;
        for file in files.flatten() {
            let Some(name) = file.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.contains(".tmp") {
                let _ = fs::remove_file(file.path());
                continue;
            }
            if !cache_name(&name) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(file.path()) else {
                continue;
            };
            if !meta.is_file() {
                let _ = fs::remove_file(file.path());
                continue;
            }
            let max_size = MAX_FILE
                + if !name.starts_with("avatar-") {
                    HEADER_LEN
                } else {
                    0
                };
            if (meta.len() == 0 || meta.len() > max_size as u64)
                && fs::remove_file(file.path()).is_ok()
            {
                continue;
            }
            // The header dates the fetch. The mtime instead records the last use.
            let fetched = if expires(&name) {
                match read_header(&file.path()) {
                    Some(at) if !lapsed(at, now) => Some(at),
                    _ => {
                        let _ = fs::remove_file(file.path());
                        dropped += 1;
                        continue;
                    }
                }
            } else {
                None
            };
            found.push((meta.modified().unwrap_or(UNIX_EPOCH), name, meta.len(), fetched));
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        for (mtime, name, size, fetched) in found {
            index.insert(name, size, mtime, fetched);
        }
        let Some(cap) = byte_cap(self.limits.bytes, free_bytes(&root), index.bytes) else {
            plx_base::eventlog::log(&format!(
                "imgcache: {} has too little free space; disk cache unavailable",
                root.display()
            ));
            index.root = None;
            return;
        };
        index.cap = Some(cap);
        self.make_room(index, None, 0, 0);
        self.publish_size(index);
        plx_base::eventlog::log(&format!(
            "imgcache: {} entries={} bytes={} cap={} expired_dropped={dropped}",
            root.display(),
            index.entries.len(),
            index.bytes,
            index.cap.unwrap_or(self.limits.bytes)
        ));
    }

    fn read_at(&self, generation: u64, key: &DiskKey) -> Option<CachedImage> {
        let mut index = self.index.lock().unwrap_or_else(|e| e.into_inner());
        if self.generation.load(Ordering::Acquire) != generation {
            return None;
        }
        self.initialize(&mut index);
        self.sweep_expired(&mut index, SystemTime::now());
        let result = self.read_locked(&mut index, key);
        if result.is_some() {
            &self.counters.hit
        } else {
            &self.counters.miss
        }
        .fetch_add(1, Ordering::Relaxed);
        result
    }

    fn read_locked(&self, index: &mut Index, key: &DiskKey) -> Option<CachedImage> {
        let root = index.root.clone()?;
        let now = SystemTime::now();
        if let Some(prior) = &key.prior {
            if !index.entries.contains_key(&key.name) && index.entries.contains_key(prior) {
                self.adopt_prior(index, prior, &key.name, unix_seconds(now));
            }
        }
        if let Some(entry) = index.entries.get(&key.name) {
            // Expired files need not be read into memory just to be discarded.
            if let Some(fetched) = entry.fetched {
                let at = unix_seconds(now);
                if !servable(fetched, at) {
                    if lapsed(fetched, at) {
                        self.remove_locked(index, &key.name);
                    }
                    return None;
                }
            }
            match read_file(&root.join(&key.name), false) {
                Some((bytes, fetched)) => {
                    let at = unix_seconds(now);
                    if expires(&key.name) && !servable(fetched, at) {
                        if lapsed(fetched, at) {
                            self.remove_locked(index, &key.name);
                        }
                        return None;
                    }
                    index.touch(&key.name, now);
                    return Some(CachedImage {
                        bytes,
                        stale: stale(fetched, now),
                    });
                }
                None => {
                    self.remove_locked(index, &key.name);
                }
            }
        }
        let legacy = key.legacy.as_ref()?;
        if !index.entries.contains_key(legacy) {
            return None;
        }
        let Some((bytes, fetched)) = read_file(&root.join(legacy), true) else {
            self.remove_locked(index, legacy);
            return None;
        };
        // Preserve the old refresh clock. A migration is not a successful server refresh.
        if self.write_locked(index, key, &bytes, fetched) {
            self.remove_locked(index, legacy);
        }
        Some(CachedImage {
            bytes,
            stale: stale(fetched, now),
        })
    }

    fn adopt_prior(&self, index: &mut Index, prior: &str, name: &str, now: u64) {
        let Some(root) = index.root.as_ref() else { return };
        let Some(old) = index.entries.get(prior) else { return };
        let (size, touched, fetched) = (old.size, old.touched, old.fetched);
        if !fetched.is_some_and(|at| servable(at, now)) {
            if fetched.is_none_or(|at| lapsed(at, now)) {
                self.remove_locked(index, prior);
            }
            return;
        }
        if fs::rename(root.join(prior), root.join(name)).is_err() {
            return;
        }
        index.forget(prior);
        index.insert(name.to_owned(), size, touched, fetched.filter(|_| expires(name)));
    }

    /// Checks the indexed root in memory; checks other candidates in case the writable root moved.
    fn sweep_expired(&self, index: &mut Index, now: SystemTime) {
        if index.swept.is_some_and(|at| now.duration_since(at).unwrap_or_default() < SWEEP_AFTER) {
            return;
        }
        index.swept = Some(now);
        let secs = unix_seconds(now);
        let due: Vec<String> = index
            .entries
            .iter()
            .filter(|(_, entry)| entry.fetched.is_some_and(|at| lapsed(at, secs)))
            .map(|(name, _)| name.clone())
            .collect();
        for name in due {
            self.remove_locked(index, &name);
        }
        self.sweep_other_roots(index.root.as_deref(), secs);
    }

    fn sweep_other_roots(&self, active: Option<&Path>, now: u64) {
        use std::os::unix::fs::MetadataExt;
        let owner = unsafe { libc::geteuid() };
        for root in self.candidates.iter().map(PathBuf::as_path).filter(|root| Some(*root) != active) {
            if !fs::symlink_metadata(root).is_ok_and(|meta| meta.is_dir() && meta.uid() == owner) {
                continue;
            }
            let Ok(files) = fs::read_dir(root) else { continue };
            for file in files.flatten() {
                let name = file.file_name();
                let Some(name) = name.to_str() else { continue };
                if !expires(name) || !cache_name(name) || !file.file_type().is_ok_and(|kind| kind.is_file()) {
                    continue;
                }
                let path = file.path();
                if read_header(&path).is_none_or(|fetched| lapsed(fetched, now)) {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }

    fn write_at(&self, generation: u64, key: &DiskKey, bytes: &[u8]) -> bool {
        let mut index = self.index.lock().unwrap_or_else(|e| e.into_inner());
        if self.generation.load(Ordering::Acquire) != generation {
            return false;
        }
        self.initialize(&mut index);
        self.sweep_expired(&mut index, SystemTime::now());
        self.write_locked(&mut index, key, bytes, unix_seconds(SystemTime::now()))
    }

    fn write_locked(&self, index: &mut Index, key: &DiskKey, bytes: &[u8], fetched: u64) -> bool {
        if bytes.is_empty() || bytes.len() > MAX_FILE {
            return false;
        }
        let Some(root) = index.root.clone() else {
            return false;
        };
        let size = (bytes.len() + HEADER_LEN) as u64;
        if size > self.cap(index) || self.limits.entries == 0 {
            return false;
        }
        // One writer under the gate, and startup removes residue from an interrupted process.
        let tmp = root.join(format!("{}.tmp", key.name));
        let dst = root.join(&key.name);
        let write = || -> std::io::Result<()> {
            let mut f = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
            f.write_all(MAGIC)?;
            f.write_all(&fetched.to_le_bytes())?;
            f.write_all(&(bytes.len() as u64).to_le_bytes())?;
            f.write_all(bytes)
        };
        if write().is_err() {
            let _ = fs::remove_file(&tmp);
            return false;
        }
        let old_size = index.entries.get(&key.name).map_or(0, |e| e.size);
        let added_entries = usize::from(!index.entries.contains_key(&key.name));
        let enough = self.make_room(
            index,
            Some(&key.name),
            size.saturating_sub(old_size),
            added_entries,
        );
        // A smaller replacement can only reduce bytes; conservatively make_room need not
        // account for that saving until the replacement actually succeeds.
        if !enough || fs::rename(&tmp, &dst).is_err() {
            let _ = fs::remove_file(&tmp);
            self.publish_size(index);
            return false;
        }
        index.forget(&key.name);
        let scoped = expires(&key.name).then_some(fetched);
        index.insert(key.name.clone(), size, SystemTime::now(), scoped);
        self.counters.write.fetch_add(1, Ordering::Relaxed);
        self.publish_size(index);
        true
    }

    /// Failed unlinks remain accounted for. If capacity cannot be freed, decline the write.
    fn make_room(
        &self,
        index: &mut Index,
        protected: Option<&str>,
        added_bytes: u64,
        added_entries: usize,
    ) -> bool {
        while index.bytes.saturating_add(added_bytes) > self.cap(index)
            || index.entries.len() + added_entries > self.limits.entries
        {
            let candidate = index
                .lru
                .iter()
                .find(|(_, name)| Some(name.as_str()) != protected)
                .map(|(_, name)| name.clone());
            let Some(name) = candidate else { return false };
            if !self.remove_locked(index, &name) {
                return false;
            }
            self.counters.eviction.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    fn remove_at(&self, generation: u64, key: &DiskKey) {
        let mut index = self.index.lock().unwrap_or_else(|e| e.into_inner());
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        self.initialize(&mut index);
        self.remove_locked(&mut index, &key.name);
        if let Some(prior) = &key.prior {
            self.remove_locked(&mut index, prior);
        }
        if let Some(legacy) = &key.legacy {
            self.remove_locked(&mut index, legacy);
        }
    }

    fn remove_locked(&self, index: &mut Index, name: &str) -> bool {
        let Some(root) = index.root.as_ref() else {
            return false;
        };
        match fs::remove_file(root.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
        index.forget(name);
        self.publish_size(index);
        true
    }

    fn clear(&self) {
        let mut index = self.index.lock().unwrap_or_else(|e| e.into_inner());
        // A tile write is one more thing that can be in flight: holding its gate across the bump
        // and the sweep lets one that began under the old generation finish before the sweep, and
        // refuses any that begins after it.
        let mut tiles = self.tiles.lock().unwrap_or_else(|e| e.into_inner());
        self.generation.fetch_add(1, Ordering::AcqRel);
        // Sweep all candidates even when this process has never initialized the cache. A
        // previous install may have fallen back to a different writable location.
        for root in &self.candidates {
            let Ok(files) = fs::read_dir(root) else {
                continue;
            };
            for file in files.flatten() {
                let name = file.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".img") || name.contains(".tmp") {
                    let _ = fs::remove_file(file.path());
                }
            }
        }
        *index = Index::default();
        *tiles = tiles::Gate::default();
        self.publish_size(&index);
    }

    fn publish_size(&self, index: &Index) {
        self.counters
            .entries
            .store(index.entries.len() as u64, Ordering::Relaxed);
        self.counters.bytes.store(index.bytes, Ordering::Relaxed);
    }

    fn warm(&self) {
        let mut index = self.index.lock().unwrap_or_else(|e| e.into_inner());
        self.initialize(&mut index);
        self.sweep_expired(&mut index, SystemTime::now());
    }

    fn stats(&self) -> Snapshot {
        let c = &self.counters;
        Snapshot {
            hit: c.hit.load(Ordering::Relaxed),
            miss: c.miss.load(Ordering::Relaxed),
            write: c.write.load(Ordering::Relaxed),
            eviction: c.eviction.load(Ordering::Relaxed),
            entries: c.entries.load(Ordering::Relaxed),
            bytes: c.bytes.load(Ordering::Relaxed),
        }
    }
}

impl Index {
    fn insert(&mut self, name: String, size: u64, touched: SystemTime, fetched: Option<u64>) {
        self.sequence += 1;
        self.bytes += size;
        self.lru.insert((self.sequence, name.clone()));
        self.entries.insert(
            name,
            Entry {
                size,
                order: self.sequence,
                touched,
                fetched,
            },
        );
    }

    fn forget(&mut self, name: &str) {
        if let Some(entry) = self.entries.remove(name) {
            self.bytes -= entry.size;
            self.lru.remove(&(entry.order, name.into()));
        }
    }

    fn touch(&mut self, name: &str, now: SystemTime) {
        let Some(entry) = self.entries.get_mut(name) else {
            return;
        };
        self.lru.remove(&(entry.order, name.into()));
        self.sequence += 1;
        entry.order = self.sequence;
        self.lru.insert((entry.order, name.into()));
        if now.duration_since(entry.touched).unwrap_or_default() >= TOUCH_AFTER {
            if let Some(root) = &self.root {
                if File::options()
                    .write(true)
                    .open(root.join(name))
                    .and_then(|f| f.set_modified(now))
                    .is_ok()
                {
                    entry.touched = now;
                }
            }
        }
    }
}

fn read_file(path: &Path, legacy: bool) -> Option<(Vec<u8>, u64)> {
    let max = MAX_FILE + if legacy { 0 } else { HEADER_LEN };
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > max as u64 {
        return None;
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // The bound also holds if another process replaced/grew the file after metadata().
    File::open(path)
        .ok()?
        .take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() || bytes.len() > max {
        return None;
    }
    if legacy {
        return Some((bytes, unix_seconds(meta.modified().unwrap_or(UNIX_EPOCH))));
    }
    if bytes.len() <= HEADER_LEN || &bytes[..8] != MAGIC {
        return None;
    }
    let fetched = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let length = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    if length != (bytes.len() - HEADER_LEN) as u64 {
        return None;
    }
    bytes.drain(..HEADER_LEN);
    Some((bytes, fetched))
}

fn stale(fetched: u64, now: SystemTime) -> bool {
    let now = unix_seconds(now);
    fetched > now || now - fetched >= REFRESH_AFTER.as_secs()
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cache_name(name: &str) -> bool {
    name.strip_prefix(PLEX_PREFIX)
        .or_else(|| name.strip_prefix(TMDB_PREFIX))
        .or_else(|| name.strip_prefix(UNSCOPED_PREFIX))
        .or_else(|| name.strip_prefix("avatar-"))
        .and_then(|s| s.strip_suffix(".img"))
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn expires(name: &str) -> bool {
    name.starts_with(TMDB_PREFIX) || name.starts_with(UNSCOPED_PREFIX)
}

/// Whether a picture's source address is on a TMDB host (`image.tmdb.org`, `www.themoviedb.org`,
/// any subdomain of `tmdb.org` or `themoviedb.org`): the data TMDB's six-month limit covers.
fn is_tmdb_source(source: &str) -> bool {
    let Some((_, rest)) = source.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").trim_end_matches('.').to_ascii_lowercase();
    ["tmdb.org", "themoviedb.org"]
        .iter()
        .any(|domain| host == *domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.')))
}

/// Whether a TMDB entry fetched at `fetched` is provably past the retention limit at `now`.
fn lapsed(fetched: u64, now: u64) -> bool {
    now >= fetched && now - fetched >= plx_base::retention::TMDB_MAX_AGE_SECS
}

/// Whether a TMDB entry may be served: not lapsed, and not dated in the future (a clock set back
/// leaves its age unknown, and an unknown age cannot be shown to be inside the limit).
fn servable(fetched: u64, now: u64) -> bool {
    fetched <= now && !lapsed(fetched, now)
}

/// The fetch time in a file's header, without reading its picture.
fn read_header(path: &Path) -> Option<u64> {
    let mut head = [0u8; HEADER_LEN];
    File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != MAGIC {
        return None;
    }
    Some(u64::from_le_bytes(head[8..16].try_into().ok()?))
}

impl Cache {
    fn cap(&self, index: &Index) -> u64 {
        index.cap.unwrap_or(self.limits.bytes)
    }
}

/// Below this much spare space the disk tier is not worth having, and taking any of it risks the
/// filesystem the television installs apps onto.
const MIN_CAP: u64 = 8 * 1024 * 1024;

/// The byte ceiling for a root: `limit`, but never more than HALF of what the filesystem could
/// give the cache (its free space plus what the cache already holds). `None` when that half is
/// under [`MIN_CAP`], or the free space is unknown and `limit` would then be a guess. The floor
/// is about the FILESYSTEM, never the configured limit, which may legitimately be small.
///
/// A 2016 set on webOS 3.4.3 has exactly one writable persistent directory a Developer Mode app
/// can reach, `/media/developer/temp`, on the 636 MB partition that also holds installed apps,
/// with 76.5 MB free: the fixed 128 MiB ceiling would have filled it.
fn byte_cap(limit: u64, free: Option<u64>, held: u64) -> Option<u64> {
    let spare = free?.saturating_add(held) / 2;
    (spare >= MIN_CAP).then_some(limit.min(spare))
}

/// Bytes an unprivileged process may still write on `path`'s filesystem, or `None` if unknown.
fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)] // the field widths differ between targets
    Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

/// Creates `path` owner-only, or tightens it to that, and says whether it now is one. What stands
/// at the name is looked at before anything is chmod'ed: the parent is world-writable, so another
/// process can plant a symlink there or pre-create the directory under its own uid. So `mkdir`
/// (never `create_dir_all`, which follows a link at the final component), then open it with
/// `O_NOFOLLOW` and `fchmod` THAT descriptor once `fstat` says it is a directory `owner` owns.
fn private_dir(path: &Path, owner: u32) -> bool {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::DirBuilder::new().mode(0o700).create(path);
    let Ok(dir) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    else {
        return false;
    };
    let Ok(meta) = dir.metadata() else {
        return false;
    };
    meta.is_dir()
        && meta.uid() == owner
        && (meta.mode() & 0o077 == 0
            || dir.set_permissions(fs::Permissions::from_mode(0o700)).is_ok())
}

fn probe_dir(path: &Path, private: bool) -> bool {
    let made = if private {
        private_dir(path, unsafe { libc::geteuid() })
    } else {
        fs::create_dir_all(path).is_ok()
    };
    if !made {
        return false;
    }
    let probe = path.join(".probe");
    let ok = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&probe)
        .is_ok();
    let _ = fs::remove_file(probe);
    ok
}

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| {
        Cache::new(
            plx_base::paths::image_cache_candidates(),
            Limits {
                bytes: MAX_BYTES,
                entries: MAX_ENTRIES,
            },
        )
    })
}

pub fn generation() -> u64 {
    cache().generation.load(Ordering::Acquire)
}
/// Runs the directory scan once, so the first poster read does not pay for it, and runs scheduled
/// expiry on subsequent calls. Call from a background thread, never the frame thread.
pub fn prewarm() {
    cache().warm();
}
pub fn read_at(generation: u64, key: &DiskKey) -> Option<CachedImage> {
    cache().read_at(generation, key)
}
pub fn write_at(generation: u64, key: &DiskKey, bytes: &[u8]) -> bool {
    cache().write_at(generation, key, bytes)
}
pub fn remove_at(generation: u64, key: &DiskKey) {
    cache().remove_at(generation, key);
}
pub fn clear() {
    cache().clear();
}
/// Lock-free and does not initialize the index: safe for frame-loop diagnostics.
pub fn stats() -> Snapshot {
    cache().stats()
}

pub mod tiles;

#[cfg(test)]
mod tests;
