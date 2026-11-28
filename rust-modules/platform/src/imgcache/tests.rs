use super::*;

const AVATAR: &str = "/photo/:/transcode?width=300&height=300&minSize=1&url=https%3A%2F%2Fplex.tv%2Fusers%2F0123abcd%2Favatar%3Fc%3D1700000000&X-Plex-Token=tok";
const POSTER: &str = "/photo/:/transcode?width=250&height=375&minSize=1&url=%2Flibrary%2Fmetadata%2F42%2Fthumb%2F1&X-Plex-Token=t";

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "plx-imgcache-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn cache(&self, entries: usize, bytes: u64) -> Cache {
        Cache::new(vec![self.0.clone()], Limits { entries, bytes })
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn poster(id: usize) -> DiskKey {
    classify(
        "server-a",
        &POSTER.replace("%2F42%2F", &format!("%2F{id}%2F")),
    )
    .unwrap()
}

#[test]
fn library_art_has_a_durable_disk_key() {
    assert!(classify("server-a", POSTER).is_some());
}

/// A scrub-preview frame is one still out of a part's BIF index, asked for once per scrub step and
/// never again: written to disk, a single scrub through a film would push hundreds of entries
/// through an LRU whose 8192 slots exist to keep posters.
#[test]
fn a_scrub_preview_frame_never_reaches_the_disk() {
    let direct = "/library/parts/7459/indexes/sd/600000?X-Plex-Token=t";
    assert!(classify("server-a", direct).is_none());
    let frame = "/photo/:/transcode?width=320&height=180&minSize=1&url=%2Flibrary%2Fparts%2F7459%2Findexes%2Fsd%2F600000&X-Plex-Token=t";
    assert!(classify("server-a", frame).is_none());
    let lower = "/photo/:/transcode?width=320&height=180&minSize=1&url=%2flibrary%2fparts%2f7459%2findexes%2fsd%2f0&X-Plex-Token=t";
    assert!(classify("server-a", lower).is_none());
    // the part's own art is not a preview frame
    let part_thumb = "/photo/:/transcode?width=320&height=180&minSize=1&url=%2Flibrary%2Fparts%2F7459%2Fthumb&X-Plex-Token=t";
    assert!(classify("server-a", part_thumb).is_some());
}

#[test]
fn keys_include_namespace_source_version_and_every_transform() {
    let name = classify("server-a", POSTER).unwrap().name;
    assert_ne!(name, classify("server-b", POSTER).unwrap().name);
    for key in [
        POSTER.replace("width=250", "width=251"),
        POSTER.replace("height=375", "height=376"),
        POSTER.replace("minSize=1", "minSize=0"),
        POSTER.replace("thumb%2F1", "thumb%2F2"),
        POSTER.replace("thumb%2F1", "thumb%2F1%3Fv%3D2"),
        format!("{POSTER}&format=png"),
        format!("{POSTER}&upscale=0"),
        format!("{POSTER}&quality=90"),
    ] {
        assert_ne!(name, classify("server-a", &key).unwrap().name, "{key}");
    }
    assert_eq!(
        name,
        classify(
            "server-a",
            &POSTER.replace("X-Plex-Token=t", "X-Plex-Token=another")
        )
        .unwrap()
        .name
    );
    let shuffled = "/photo/:/transcode?X-Plex-Token=rotated&minSize=1&height=375&url=%2flibrary%2fmetadata%2f42%2fthumb%2f1&width=250";
    assert_eq!(name, classify("server-a", shuffled).unwrap().name);
    assert!(name.starts_with(PLEX_PREFIX) && name.ends_with(".img"));
    assert!(cache_name(&name));
    for bad in [
        "no-query",
        "/other?url=image",
        "/photo/:/transcode?width=1",
        "/photo/:/transcode?url=",
        "/photo/:/transcode?url=a&url=b",
    ] {
        assert!(classify("server-a", bad).is_none());
    }
    assert!(classify("", POSTER).is_none());
}

#[test]
fn baked_keys_are_stable_versioned_by_stamp_and_carry_no_request() {
    let key = |ns: &str, kind: &str, rk: &str, stamp: &str, w: u32, h: u32| {
        classify_baked(ns, kind, rk, stamp, w, h).map(|k| k.name)
    };
    let name = key("machine:m|profile:", "fan", "901", "1700000000", 300, 450).unwrap();
    assert_eq!(
        Some(name.clone()),
        key("machine:m|profile:", "fan", "901", "1700000000", 300, 450),
        "the same inputs must name the same file on every boot"
    );
    assert!(cache_name(&name), "baked files must survive the startup scan: {name}");
    for other in [
        key("machine:n|profile:", "fan", "901", "1700000000", 300, 450),
        key("machine:m|profile:u1", "fan", "901", "1700000000", 300, 450),
        key("machine:m|profile:", "other", "901", "1700000000", 300, 450),
        key("machine:m|profile:", "fan", "902", "1700000000", 300, 450),
        key("machine:m|profile:", "fan", "901", "1700000001", 300, 450),
        key("machine:m|profile:", "fan", "901", "1700000000", 301, 450),
        key("machine:m|profile:", "fan", "901", "1700000000", 300, 451),
    ] {
        assert_ne!(Some(name.clone()), other);
    }
    // A baked identity never collides with a transcode of the same source path.
    assert_ne!(
        Some(name.clone()),
        classify("machine:m|profile:", POSTER).map(|k| k.name)
    );
    for (ns, kind, rk, stamp, w, h) in [
        ("", "fan", "1", "2", 3, 4),
        ("n", "", "1", "2", 3, 4),
        ("n", "fan", "", "2", 3, 4),
        ("n", "fan", "1", "", 3, 4),
        ("n", "fan", "1", "2", 0, 4),
        ("n", "fan", "1", "2", 3, 0),
    ] {
        assert!(key(ns, kind, rk, stamp, w, h).is_none());
    }
}

#[test]
fn avatar_only_ignores_roster_stamp_and_preserves_other_queries() {
    let key = classify("server-a", AVATAR).unwrap();
    assert_eq!(
        key,
        classify("server-a", &AVATAR.replace("1700000000", "1700000001")).unwrap()
    );
    assert_eq!(
        key,
        classify("server-a", &AVATAR.replace("%3Fc%3D1700000000", "")).unwrap()
    );
    assert_ne!(
        key.name,
        classify(
            "server-a",
            &AVATAR.replace("%3Fc%3D1700000000", "%3Fc%3D1700000000%26version%3D2")
        )
        .unwrap()
        .name
    );
    let other = AVATAR.replace("plex.tv", "metadata-static.plex.tv");
    assert_ne!(
        classify("server-a", &other).unwrap().name,
        classify("server-a", &other.replace("1700000000", "1700000001"))
            .unwrap()
            .name
    );
    assert!(classify("server-a", &format!("{AVATAR}&format=png"))
        .unwrap()
        .legacy
        .is_none());
    assert_eq!(
        percent_decode("plus+stays%2Bbad%zz%2"),
        "plus+stays+bad%zz%2"
    );
}

#[test]
fn more_than_a_thousand_images_survive_reopen_without_network() {
    let dir = TestDir::new();
    let cache = dir.cache(MAX_ENTRIES, MAX_BYTES);
    for id in 0..1200 {
        assert!(cache.write_at(0, &poster(id), &id.to_le_bytes()));
    }
    assert_eq!(cache.stats().entries, 1200);
    drop(cache);
    let reopened = dir.cache(MAX_ENTRIES, MAX_BYTES);
    for id in 0..1200 {
        let hit = reopened.read_at(0, &poster(id)).unwrap();
        assert_eq!(hit.bytes, id.to_le_bytes());
        assert!(!hit.stale);
    }
    let stats = reopened.stats();
    assert_eq!(stats.hit, 1200);
    assert_eq!(stats.miss, 0);
    assert_eq!(stats.write, 0);
    assert_eq!(stats.entries, 1200);
    assert_eq!(
        stats.bytes,
        1200 * (HEADER_LEN + std::mem::size_of::<usize>()) as u64
    );
}

#[test]
fn count_limit_evicts_least_recently_read_and_not_the_recent_hit() {
    let dir = TestDir::new();
    let cache = dir.cache(3, MAX_BYTES);
    for id in 0..3 {
        assert!(cache.write_at(0, &poster(id), b"data"));
    }
    assert!(cache.read_at(0, &poster(0)).is_some());
    assert!(cache.write_at(0, &poster(3), b"next"));
    assert!(cache.read_at(0, &poster(1)).is_none());
    for id in [0, 2, 3] {
        assert!(cache.read_at(0, &poster(id)).is_some());
    }
    assert_eq!(cache.stats().eviction, 1);
    assert_eq!(cache.stats().entries, 3);
}

#[test]
fn byte_limit_and_replacement_account_for_header_and_old_size() {
    let dir = TestDir::new();
    let cache = dir.cache(10, 2 * (HEADER_LEN + 20) as u64);
    assert!(cache.write_at(0, &poster(1), &[1; 20]));
    assert!(cache.write_at(0, &poster(2), &[2; 20]));
    assert!(cache.write_at(0, &poster(2), &[3; 30]));
    assert!(cache.read_at(0, &poster(1)).is_none());
    assert_eq!(cache.stats().bytes, (HEADER_LEN + 30) as u64);
    assert!(cache.write_at(0, &poster(2), &[4; 2]));
    assert_eq!(cache.stats().bytes, (HEADER_LEN + 2) as u64);
    assert!(!cache.write_at(0, &poster(3), &[0; 100]));
    assert_eq!(cache.stats().entries, 1);
    assert_eq!(cache.read_at(0, &poster(2)).unwrap().bytes, [4; 2]);
}

#[test]
fn startup_enforces_new_limits_and_recovers_orphan_temporary_files() {
    let dir = TestDir::new();
    let initial = dir.cache(20, MAX_BYTES);
    for id in 0..10 {
        assert!(initial.write_at(0, &poster(id), b"image"));
        File::options()
            .write(true)
            .open(dir.0.join(&poster(id).name))
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(100 + id as u64))
            .unwrap();
    }
    fs::write(
        dir.0.join(format!("{}.tmp123-4", poster(0).name)),
        b"partial",
    )
    .unwrap();
    drop(initial);
    let cache = dir.cache(4, 3 * (HEADER_LEN + 5) as u64);
    assert!(cache.read_at(0, &poster(9)).is_some());
    assert_eq!(cache.stats().entries, 3);
    assert_eq!(cache.stats().eviction, 7);
    assert!(cache.read_at(0, &poster(6)).is_none());
    assert!(fs::read_dir(&dir.0).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".tmp")));
}

#[test]
fn reads_do_not_refresh_fetch_time_and_recency_survives_reopen() {
    let dir = TestDir::new();
    let cache = dir.cache(2, MAX_BYTES);
    let old = unix_seconds(SystemTime::now()) - REFRESH_AFTER.as_secs() - 1;
    {
        let mut index = cache.index.lock().unwrap();
        cache.initialize(&mut index);
        for id in 0..2 {
            assert!(cache.write_locked(&mut index, &poster(id), b"data", old));
            let stamp = UNIX_EPOCH + Duration::from_secs(old + id as u64);
            File::options()
                .write(true)
                .open(dir.0.join(&poster(id).name))
                .unwrap()
                .set_modified(stamp)
                .unwrap();
            index.entries.get_mut(&poster(id).name).unwrap().touched = stamp;
        }
    }
    assert!(cache.read_at(0, &poster(0)).unwrap().stale);
    assert_eq!(
        read_file(&dir.0.join(&poster(0).name), false).unwrap().1,
        old
    );
    let touched = fs::metadata(dir.0.join(&poster(0).name))
        .unwrap()
        .modified()
        .unwrap();
    assert!(touched > UNIX_EPOCH + Duration::from_secs(old));
    assert!(cache.read_at(0, &poster(0)).unwrap().stale);
    assert_eq!(
        fs::metadata(dir.0.join(&poster(0).name))
            .unwrap()
            .modified()
            .unwrap(),
        touched,
        "do not write metadata on every hit"
    );
    drop(cache);
    let reopened = dir.cache(2, MAX_BYTES);
    assert!(reopened.write_at(0, &poster(2), b"new"));
    assert!(reopened.read_at(0, &poster(1)).is_none());
    assert!(reopened.read_at(0, &poster(0)).unwrap().stale);
}

#[test]
fn corrupt_empty_oversized_and_truncated_files_are_misses() {
    let dir = TestDir::new();
    let cache = dir.cache(10, MAX_BYTES);
    assert!(!cache.write_at(0, &poster(0), b""));
    assert!(!cache.write_at(0, &poster(0), &vec![0; MAX_FILE + 1]));
    for id in 0..4 {
        assert!(cache.write_at(0, &poster(id), b"some bytes"));
    }
    fs::write(dir.0.join(&poster(0).name), b"not a valid header").unwrap();
    File::options()
        .write(true)
        .open(dir.0.join(&poster(1).name))
        .unwrap()
        .set_len((MAX_FILE + HEADER_LEN + 1) as u64)
        .unwrap();
    File::options()
        .write(true)
        .open(dir.0.join(&poster(2).name))
        .unwrap()
        .set_len(0)
        .unwrap();
    File::options()
        .write(true)
        .open(dir.0.join(&poster(3).name))
        .unwrap()
        .set_len((HEADER_LEN + 1) as u64)
        .unwrap();
    for id in 0..4 {
        assert!(cache.read_at(0, &poster(id)).is_none());
    }
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().bytes, 0);
    // Oversized residue present before initialization is bounded and removed too.
    File::create(dir.0.join(&poster(5).name))
        .unwrap()
        .set_len((MAX_FILE * 4) as u64)
        .unwrap();
    let reopened = dir.cache(10, MAX_BYTES);
    assert!(reopened.read_at(0, &poster(5)).is_none());
    assert_eq!(reopened.stats().entries, 0);
}

#[test]
fn legacy_avatar_migrates_without_resetting_refresh_clock() {
    let dir = TestDir::new();
    let key = classify("server-a", AVATAR).unwrap();
    let legacy = dir.0.join(key.legacy.as_ref().unwrap());
    fs::write(&legacy, b"legacy image").unwrap();
    File::options()
        .write(true)
        .open(&legacy)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(100))
        .unwrap();
    let cache = dir.cache(100, MAX_BYTES);
    let hit = cache.read_at(0, &key).unwrap();
    assert_eq!(hit.bytes, b"legacy image");
    assert!(hit.stale);
    assert!(!legacy.exists());
    assert!(dir.0.join(&key.name).exists());
    assert_eq!(read_file(&dir.0.join(&key.name), false).unwrap().1, 100);
    assert_eq!(cache.stats().entries, 1);
}

#[test]
fn clear_scrubs_all_candidates_and_rejects_old_reads_writes_and_removes() {
    let primary = TestDir::new();
    let fallback = TestDir::new();
    let cache = Cache::new(
        vec![primary.0.clone(), fallback.0.clone()],
        Limits {
            entries: 10,
            bytes: MAX_BYTES,
        },
    );
    assert!(cache.write_at(0, &poster(0), b"old account"));
    fs::write(fallback.0.join(&poster(1).name), b"other location").unwrap();
    fs::write(fallback.0.join("avatar-old.img.tmp123"), b"partial").unwrap();
    fs::write(fallback.0.join("keep.txt"), b"unrelated").unwrap();
    cache.clear();
    assert_eq!(cache.generation.load(Ordering::Acquire), 1);
    assert!(!fallback.0.join(&poster(1).name).exists());
    assert!(!fallback.0.join("avatar-old.img.tmp123").exists());
    assert!(fallback.0.join("keep.txt").exists());
    assert!(!cache.write_at(0, &poster(0), b"late fetch"));
    assert!(cache.write_at(1, &poster(0), b"new account"));
    assert!(cache.read_at(0, &poster(0)).is_none());
    cache.remove_at(0, &poster(0));
    assert_eq!(cache.read_at(1, &poster(0)).unwrap().bytes, b"new account");
}

#[test]
fn concurrent_clear_waits_for_gate_and_late_write_is_rejected() {
    let dir = TestDir::new();
    let cache = std::sync::Arc::new(dir.cache(10, MAX_BYTES));
    assert!(cache.write_at(0, &poster(0), b"old"));
    let held = cache.index.lock().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let worker_cache = cache.clone();
    let clear = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        worker_cache.clear();
    });
    started_rx.recv().unwrap();
    assert_eq!(cache.generation.load(Ordering::Acquire), 0);
    drop(held);
    clear.join().unwrap();
    assert!(!cache.write_at(0, &poster(0), b"late"));
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn failed_atomic_write_keeps_previous_entry_and_accurate_accounting() {
    let dir = TestDir::new();
    let cache = dir.cache(10, MAX_BYTES);
    assert!(cache.write_at(0, &poster(0), b"original"));
    let before = cache.stats();
    fs::create_dir(dir.0.join(format!("{}.tmp", poster(0).name))).unwrap();
    assert!(!cache.write_at(0, &poster(0), b"replacement"));
    assert_eq!(cache.stats().bytes, before.bytes);
    assert_eq!(cache.stats().entries, before.entries);
    assert_eq!(cache.stats().write, before.write);
    assert_eq!(cache.read_at(0, &poster(0)).unwrap().bytes, b"original");
}

#[test]
fn unavailable_storage_is_harmless_and_stats_do_not_initialize_it() {
    let dir = TestDir::new();
    let file = dir.0.join("regular-file");
    fs::write(&file, b"cannot create directory here").unwrap();
    let cache = Cache::new(
        vec![file],
        Limits {
            entries: 10,
            bytes: MAX_BYTES,
        },
    );
    assert_eq!(cache.stats().entries, 0);
    assert!(!cache.index.lock().unwrap().initialized);
    assert!(cache.read_at(0, &poster(0)).is_none());
    assert!(!cache.write_at(0, &poster(0), b"image"));
    assert_eq!(cache.stats().miss, 1);
}

/// The 3.4.3 set's numbers: 76.5 MB free on the partition installed apps share, so the cache takes
/// at most half, not its fixed 128 MiB.
#[test]
fn the_byte_ceiling_takes_at_most_half_of_what_the_filesystem_can_spare() {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    assert_eq!(byte_cap(128 * MB, Some(76 * MB), 0), Some(38 * MB));
    // A roomy disk keeps the fixed ceiling.
    assert_eq!(byte_cap(128 * MB, Some(10 * GB), 0), Some(128 * MB));
    // What the cache already holds counts as spare.
    assert_eq!(byte_cap(128 * MB, Some(40 * MB), 36 * MB), Some(38 * MB));
    // Too little to be worth having, and unknown free space is not a licence.
    assert_eq!(byte_cap(128 * MB, Some(12 * MB), 0), None);
    assert_eq!(byte_cap(128 * MB, None, 0), None);
    // A small configured limit is honoured.
    assert_eq!(byte_cap(4096, Some(10 * GB), 0), Some(4096));
}

#[test]
fn a_test_directory_reports_its_free_space() {
    let dir = TestDir::new();
    assert!(free_bytes(&dir.0).is_some_and(|b| b > 0));
}

/// `/media/developer/temp` is mode 777, so a cache directory made there with the default mode
/// would let any other process read the library's posters.
#[test]
fn a_private_root_is_created_for_its_owner_alone() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = TestDir::new();
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o777)).unwrap();
    let root = dir.0.join("imgcache");
    assert!(probe_dir(&root, true));
    assert_eq!(fs::metadata(&root).unwrap().mode() & 0o7777, 0o700);
}

#[test]
fn a_looser_private_root_we_own_is_tightened_and_a_foreign_or_linked_one_is_refused() {
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    let dir = TestDir::new();
    let mode = |p: &Path| fs::metadata(p).unwrap().mode() & 0o7777;
    let ours = dir.0.join("ours");
    fs::create_dir(&ours).unwrap();
    fs::set_permissions(&ours, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(probe_dir(&ours, true));
    assert_eq!(mode(&ours), 0o700);
    // Not ours: left as found, and not used.
    let theirs = dir.0.join("theirs");
    fs::create_dir(&theirs).unwrap();
    fs::set_permissions(&theirs, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!private_dir(&theirs, unsafe { libc::geteuid() } + 1));
    assert_eq!(mode(&theirs), 0o755);
    // A link planted at the name must not carry the chmod, or the cache, to its target.
    let link = dir.0.join("link");
    symlink(&theirs, &link).unwrap();
    assert!(!probe_dir(&link, true));
    assert_eq!(mode(&theirs), 0o755);
}

#[test]
fn a_root_that_is_not_private_keeps_the_mode_it_has() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = TestDir::new();
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(probe_dir(&dir.0, false));
    assert_eq!(fs::metadata(&dir.0).unwrap().mode() & 0o7777, 0o755);
}

#[test]
fn the_directory_scan_can_run_ahead_of_the_first_read() {
    let dir = TestDir::new();
    let writer = dir.cache(10, MAX_BYTES);
    for i in 0..3 {
        assert!(writer.write_at(0, &poster(i), b"image"));
    }
    let cache = dir.cache(10, MAX_BYTES);
    assert_eq!(cache.stats().entries, 0);
    cache.warm();
    assert_eq!(cache.stats().entries, 3);
    assert!(cache.index.lock().unwrap().initialized);
}

// ---- TMDB retention ---------------------------------------------------------------------------

const LIMIT: u64 = plx_base::retention::TMDB_MAX_AGE_SECS;

fn now_secs() -> u64 {
    unix_seconds(SystemTime::now())
}

/// A transcode request for the picture at `source`, as the Requests posters are built.
fn transcode_of(source: &str) -> String {
    let encoded = source.replace(':', "%3A").replace('/', "%2F");
    format!("/photo/:/transcode?width=342&height=513&minSize=1&url={encoded}&X-Plex-Token=t")
}

fn tmdb(path: &str) -> DiskKey {
    classify("server-a", &transcode_of(&format!("https://image.tmdb.org/t/p/w342/{path}"))).unwrap()
}

#[test]
fn only_pictures_sourced_from_tmdb_hosts_are_scoped() {
    let key = tmdb("a.jpg");
    assert!(key.name.starts_with("tmdb-") && cache_name(&key.name), "{}", key.name);
    let prior = key.prior.as_deref().unwrap();
    assert!(prior.starts_with("image-") && prior.ends_with(&key.name["tmdb-".len()..]));
    for scoped in [
        "https://api.themoviedb.org/3/movie/1",
        "https://www.themoviedb.org/t/p/w92/a.jpg",
        "HTTPS://IMAGE.TMDB.ORG:443/t/p/w92/a.jpg",
        "https://tmdb.org/a.jpg",
    ] {
        let key = classify("server-a", &transcode_of(scoped)).unwrap();
        assert!(key.name.starts_with("tmdb-"), "{scoped}");
    }
    for plain in [
        "https://image.tmdb.org.example.com/a.jpg",
        "https://example.com/?u=image.tmdb.org",
        "https://image.tmdb.org@example.com/a.jpg",
        "https://notthemoviedb.org/a.jpg",
        "https://metadata-static.plex.tv/a.jpg",
    ] {
        let key = classify("server-a", &transcode_of(plain)).unwrap();
        assert!(key.name.starts_with(PLEX_PREFIX) && !expires(&key.name), "{plain}");
    }
    assert!(poster(1).name.starts_with(PLEX_PREFIX));
    assert!(classify_baked("m", "fan", "1", "2", 3, 4).unwrap().name.starts_with(PLEX_PREFIX));
}

#[test]
fn a_tmdb_picture_past_the_limit_is_never_served_and_plex_art_is_kept() {
    let dir = TestDir::new();
    let cache = dir.cache(100, MAX_BYTES);
    let old = now_secs() - LIMIT - 1;
    {
        let mut index = cache.index.lock().unwrap();
        cache.initialize(&mut index);
        assert!(cache.write_locked(&mut index, &tmdb("old.jpg"), b"tmdb", old));
        assert!(cache.write_locked(&mut index, &poster(1), b"plex", old));
    }
    assert!(cache.read_at(0, &tmdb("old.jpg")).is_none());
    assert!(!dir.0.join(&tmdb("old.jpg").name).exists(), "an expired picture is deleted, not kept");
    let plex = cache.read_at(0, &poster(1)).unwrap();
    assert_eq!(plex.bytes, b"plex");
    assert!(plex.stale, "Plex art has no age limit; it is only refreshed");
    assert_eq!(cache.stats().entries, 1);
}

#[test]
fn a_tmdb_picture_inside_the_limit_is_served_and_refreshed_daily() {
    let dir = TestDir::new();
    let cache = dir.cache(100, MAX_BYTES);
    {
        let mut index = cache.index.lock().unwrap();
        cache.initialize(&mut index);
        assert!(cache.write_locked(&mut index, &tmdb("a.jpg"), b"fresh", now_secs()));
        assert!(cache.write_locked(&mut index, &tmdb("b.jpg"), b"aging", now_secs() - LIMIT + 3600));
    }
    assert!(!cache.read_at(0, &tmdb("a.jpg")).unwrap().stale);
    let aging = cache.read_at(0, &tmdb("b.jpg")).unwrap();
    assert!(aging.stale, "a day old or more asks for a refresh");
    assert_eq!(aging.bytes, b"aging");
}

#[test]
fn a_future_dated_tmdb_picture_is_a_miss_but_is_left_for_the_next_write_to_replace() {
    let dir = TestDir::new();
    let cache = dir.cache(100, MAX_BYTES);
    {
        let mut index = cache.index.lock().unwrap();
        cache.initialize(&mut index);
        assert!(cache.write_locked(&mut index, &tmdb("a.jpg"), b"x", now_secs() + 86_400));
    }
    assert!(cache.read_at(0, &tmdb("a.jpg")).is_none());
    assert!(dir.0.join(&tmdb("a.jpg").name).exists());
    assert!(cache.write_at(0, &tmdb("a.jpg"), b"new"));
    assert_eq!(cache.read_at(0, &tmdb("a.jpg")).unwrap().bytes, b"new");
}

#[test]
fn startup_deletes_expired_tmdb_and_unscoped_art_but_keeps_plex_art() {
    let dir = TestDir::new();
    let fallback = TestDir::new();
    let old = now_secs() - LIMIT - 1;
    let prior = |id| DiskKey { name: poster(id).prior.unwrap(), legacy: None, prior: None };
    {
        let writer = dir.cache(100, MAX_BYTES);
        let mut index = writer.index.lock().unwrap();
        writer.initialize(&mut index);
        assert!(writer.write_locked(&mut index, &tmdb("old.jpg"), b"old", old));
        assert!(writer.write_locked(&mut index, &tmdb("new.jpg"), b"new", now_secs()));
        assert!(writer.write_locked(&mut index, &poster(1), b"plex", old));
        assert!(writer.write_locked(&mut index, &prior(2), b"old unscoped", old));
        assert!(writer.write_locked(&mut index, &prior(3), b"young unscoped", now_secs()));
    }
    {
        let writer = fallback.cache(100, MAX_BYTES);
        let mut index = writer.index.lock().unwrap();
        writer.initialize(&mut index);
        assert!(writer.write_locked(&mut index, &tmdb("abandoned.jpg"), b"old fallback", old));
        assert!(writer.write_locked(&mut index, &poster(4), b"plex fallback", old));
    }
    let cache = Cache::new(vec![dir.0.clone(), fallback.0.clone()], Limits { entries: 100, bytes: MAX_BYTES });
    cache.warm();
    assert_eq!(cache.stats().entries, 3);
    assert!(!dir.0.join(&tmdb("old.jpg").name).exists());
    assert!(dir.0.join(&tmdb("new.jpg").name).exists());
    assert!(dir.0.join(&poster(1).name).exists());
    assert!(!dir.0.join(prior(2).name).exists());
    assert_eq!(cache.read_at(0, &poster(3)).unwrap().bytes, b"young unscoped");
    assert!(!dir.0.join(prior(3).name).exists());
    assert!(cache.index.lock().unwrap().entries[&poster(3).name].fetched.is_none());
    assert!(!fallback.0.join(&tmdb("abandoned.jpg").name).exists());
    assert!(fallback.0.join(&poster(4).name).exists());
}

#[test]
fn a_running_cache_drops_idle_expired_tmdb_entries_on_its_hourly_pass() {
    let dir = TestDir::new();
    let cache = dir.cache(100, MAX_BYTES);
    let mut index = cache.index.lock().unwrap();
    cache.initialize(&mut index);
    // Written already past the limit, as if its age had run out while the process stayed up.
    assert!(cache.write_locked(&mut index, &tmdb("idle.jpg"), b"x", now_secs() - LIMIT - 1));
    assert!(cache.write_locked(&mut index, &poster(1), b"plex", now_secs() - LIMIT - 1));
    let now = SystemTime::now();
    cache.sweep_expired(&mut index, now);
    assert!(dir.0.join(&tmdb("idle.jpg").name).exists(), "the pass runs at most once an hour");
    cache.sweep_expired(&mut index, now + SWEEP_AFTER);
    assert!(!dir.0.join(&tmdb("idle.jpg").name).exists());
    assert!(dir.0.join(&poster(1).name).exists());
    assert_eq!(index.entries.len(), 1);
}

#[test]
fn a_tmdb_picture_stored_before_the_scope_is_adopted_with_its_fetch_time() {
    let dir = TestDir::new();
    let cache = dir.cache(100, MAX_BYTES);
    let key = tmdb("kept.jpg");
    let plain = |key: &DiskKey| DiskKey { name: key.prior.clone().unwrap(), legacy: None, prior: None };
    let fetched = now_secs() - 40 * 86_400;
    {
        let mut index = cache.index.lock().unwrap();
        cache.initialize(&mut index);
        assert!(cache.write_locked(&mut index, &plain(&key), b"before", fetched));
        assert!(cache.write_locked(&mut index, &plain(&tmdb("gone.jpg")), b"before", now_secs() - LIMIT - 1));
    }
    let hit = cache.read_at(0, &key).unwrap();
    assert_eq!(hit.bytes, b"before");
    assert!(!dir.0.join(key.prior.as_ref().unwrap()).exists());
    assert_eq!(read_file(&dir.0.join(&key.name), false).unwrap().1, fetched, "its age is not reset");
    let expired = tmdb("gone.jpg");
    assert!(cache.read_at(0, &expired).is_none(), "an unscoped file past the limit is not served");
    assert!(!dir.0.join(&expired.name).exists());
    assert!(!dir.0.join(expired.prior.unwrap()).exists());
    assert_eq!(cache.stats().entries, 1);
    cache.remove_at(0, &key);
    assert_eq!(cache.stats().entries, 0);
}

// ---- avatar tiles -----------------------------------------------------------------------------

fn tile_of(key: &str, fetched: u64, shade: u8) -> tiles::Tile {
    tiles::Tile::new(tiles::identify(key).unwrap(), fetched, 2, 2, &[shade; 16]).unwrap()
}

#[test]
fn a_tile_is_named_by_user_and_shape_and_versioned_by_the_stamp() {
    let id = tiles::identify(AVATAR).unwrap();
    assert_eq!(id.stamp, "1700000000");
    assert_eq!(id.name.len(), 64);
    // The version and the credential are not part of the name; the user and the size are.
    for same in [AVATAR.replace("1700000000", "1800000000"), AVATAR.replace("tok", "another")] {
        assert_eq!(tiles::identify(&same).unwrap().name, id.name, "{same}");
    }
    for other in [
        AVATAR.replace("0123abcd", "4567ef01"),
        AVATAR.replace("width=300", "width=301"),
        AVATAR.replace("height=300", "height=200"),
    ] {
        assert_ne!(tiles::identify(&other).unwrap().name, id.name, "{other}");
    }
    assert_eq!(tiles::identify(&AVATAR.replace("%3Fc%3D1700000000", "")).unwrap().stamp, "");
}

#[test]
fn only_profile_pictures_get_tiles() {
    assert!(tiles::identify(POSTER).is_none());
    let headshot = "/photo/:/transcode?width=300&height=300&url=https%3A%2F%2Fmetadata-static.plex.tv%2Fa%2Fpeople%2F0123.jpg&X-Plex-Token=t";
    assert!(tiles::identify(headshot).is_none());
    assert!(tiles::identify("/library/metadata/1/thumb?X-Plex-Token=t").is_none());
    let long = AVATAR.replace("1700000000", &"9".repeat(65));
    assert!(tiles::identify(&long).is_none(), "a stamp that long is not a stamp");
}

#[test]
fn tiles_round_trip_through_one_file_that_each_save_replaces() {
    let dir = TestDir::new();
    let cache = dir.cache(10, MAX_BYTES);
    let a = tile_of(AVATAR, 100, 7);
    let b = tile_of(&AVATAR.replace("0123abcd", "4567ef01"), 200, 9);
    assert!(cache.save_tiles(0, &[&a, &b]));
    assert_eq!(cache.load_tiles(), vec![a, b.clone()]);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1, "one file, and no temporary left behind");
    assert!(cache.save_tiles(0, &[&b]));
    assert_eq!(cache.load_tiles(), vec![b]);
    assert!(cache.save_tiles(0, &[]));
    assert!(cache.load_tiles().is_empty());
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0, "an empty set removes the file");
}

#[test]
fn a_damaged_file_loads_no_tiles() {
    let dir = TestDir::new();
    let cache = dir.cache(10, MAX_BYTES);
    let a = tile_of(AVATAR, 100, 7);
    assert!(cache.save_tiles(0, &[&a]));
    let path = dir.0.join("tiles.img");
    let good = fs::read(&path).unwrap();
    // Offsets: a 16-byte header, then the entry's 64-byte name, fetched (8), width, height.
    let patched = |at: usize, to: u8| {
        let mut bytes = good.clone();
        bytes[at] = to;
        bytes
    };
    let cases = [
        good[..good.len() - 1].to_vec(),
        [&good[..], b"x"].concat(),
        Vec::new(),
        b"not a tile file at all".to_vec(),
        patched(0, b'X'),
        patched(8, 200),
        patched(16 + 72, 0),
        patched(16, b'G'),
    ];
    for (i, bytes) in cases.iter().enumerate() {
        fs::write(&path, bytes).unwrap();
        assert!(cache.load_tiles().is_empty(), "case {i}");
    }
    fs::write(&path, &good).unwrap();
    assert_eq!(cache.load_tiles(), vec![a]);
}

#[test]
fn tiles_are_bounded_and_must_be_well_formed() {
    let dir = TestDir::new();
    let cache = dir.cache(10, MAX_BYTES);
    let many: Vec<tiles::Tile> = (0..=tiles::MAX_TILES)
        .map(|i| tile_of(&AVATAR.replace("0123abcd", &format!("user{i}")), 1, 1))
        .collect();
    let all: Vec<&tiles::Tile> = many.iter().collect();
    assert!(!cache.save_tiles(0, &all));
    assert!(cache.save_tiles(0, &all[..tiles::MAX_TILES]));
    assert_eq!(cache.load_tiles().len(), tiles::MAX_TILES);
    let mut skewed = many[0].clone();
    skewed.rgba = vec![0u8; 15].into();
    assert!(!cache.save_tiles(0, &[&skewed]));
    let id = tiles::identify(AVATAR).unwrap();
    assert!(tiles::Tile::new(id.clone(), 1, 2, 2, &[0; 15]).is_none());
    assert!(tiles::Tile::new(id, 1, 513, 1, &vec![0; 513 * 4]).is_none());
}

#[test]
fn sign_out_removes_tiles_and_refuses_a_write_begun_before_it() {
    let primary = TestDir::new();
    let fallback = TestDir::new();
    let cache = Cache::new(
        vec![primary.0.clone(), fallback.0.clone()],
        Limits {
            entries: 10,
            bytes: MAX_BYTES,
        },
    );
    let a = tile_of(AVATAR, 100, 7);
    assert!(cache.save_tiles(0, &[&a]));
    fs::copy(primary.0.join("tiles.img"), fallback.0.join("tiles.img")).unwrap();
    cache.clear();
    assert!(!primary.0.join("tiles.img").exists());
    assert!(!fallback.0.join("tiles.img").exists());
    assert!(cache.load_tiles().is_empty());
    assert!(!cache.save_tiles(0, &[&a]), "a profile picture must not come back after sign-out");
    assert!(cache.load_tiles().is_empty());
    assert!(cache.save_tiles(1, &[&a]));
}

#[test]
fn a_tile_is_due_when_its_stamp_changes_or_a_day_passes() {
    let t = tile_of(AVATAR, 1_000_000, 1);
    let day = REFRESH_AFTER.as_secs();
    assert!(!t.due("1700000000", 1_000_000 + day - 1));
    assert!(t.due("1700000000", 1_000_000 + day));
    assert!(t.due("1800000000", 1_000_000), "a newer stamp is due at once");
    assert!(t.due("1700000000", 999_999), "a clock that reads before the fetch is unconfirmed");
    assert!(tile_of(AVATAR, 0, 1).due("1700000000", 1), "never confirmed");
}
