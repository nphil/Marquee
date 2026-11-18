//! Play Version: a picked `Media[]` entry reaches every reader of the play path — the MDE ask,
//! the transcode decision and start URL, the direct-play part, the stream selection and the track
//! store — and none of them takes `Media[0]`.

use super::*;
#[allow(unused_imports)]
use super::test_support::*;

/// An episode with two versions, listed default-first: a 1080p h264/aac file and a 4K hevc/eac3
/// one, each with its own part and its own audio stream ids.
const TWO_VERSIONS: &[u8] = br#"{"MediaContainer":{"Metadata":[{"ratingKey":"rk-v","type":"episode","Media":[
  {"id":3086,"bitrate":2372,"width":1920,"height":1080,"videoResolution":"1080","videoCodec":"h264","audioCodec":"aac","container":"mkv",
   "Part":[{"id":3134,"key":"/library/parts/3134/1/file.mkv","container":"mkv","Stream":[
     {"id":31341,"streamType":1,"codec":"h264","index":0},
     {"id":31342,"streamType":2,"codec":"aac","channels":2,"index":1,"languageCode":"eng","default":true,"selected":true}]}]},
  {"id":3082,"bitrate":14663,"width":3840,"height":2160,"videoResolution":"4k","videoCodec":"hevc","audioCodec":"eac3","container":"mkv",
   "Part":[{"id":3130,"key":"/library/parts/3130/1/file.mkv","container":"mkv","Stream":[
     {"id":31301,"streamType":1,"codec":"hevc","index":0},
     {"id":31302,"streamType":2,"codec":"eac3","channels":6,"index":1,"languageCode":"eng","default":true,"selected":true}]}]}]}]}}"#;

/// A 4K first copy, then a 1080p one: a set whose decoder stops at 1080p can only play the second.
const FOUR_K_FIRST: &[u8] = br#"{"MediaContainer":{"Metadata":[{"ratingKey":"rk-4k","type":"movie","Media":[
  {"id":3082,"bitrate":14663,"width":3840,"height":2160,"videoResolution":"4k","videoCodec":"hevc","audioCodec":"eac3","container":"mkv",
   "Part":[{"id":3130,"key":"/library/parts/3130/1/file.mkv","container":"mkv","Stream":[
     {"id":31301,"streamType":1,"codec":"hevc","index":0},
     {"id":31302,"streamType":2,"codec":"eac3","channels":6,"index":1,"languageCode":"eng","default":true,"selected":true}]}]},
  {"id":3086,"bitrate":2372,"width":1920,"height":800,"videoResolution":"1080","videoCodec":"h264","audioCodec":"aac","container":"mkv",
   "Part":[{"id":3134,"key":"/library/parts/3134/1/file.mkv","container":"mkv","Stream":[
     {"id":31341,"streamType":1,"codec":"h264","index":0},
     {"id":31342,"streamType":2,"codec":"aac","channels":2,"index":1,"languageCode":"eng","default":true,"selected":true}]}]}]}]}}"#;

/// The same shape, with stream bitrates: the 1080p copy is 8540 kbps whole but 7900 of video, which
/// fits the 8 Mbps rung only when judged on its video.
const FOUR_K_THEN_A_RUNG_SIZED_1080P: &[u8] = br#"{"MediaContainer":{"Metadata":[{"ratingKey":"rk-rung","type":"movie","Media":[
  {"id":3082,"bitrate":14663,"width":3840,"height":2160,"videoResolution":"4k","videoCodec":"hevc","audioCodec":"eac3","container":"mkv",
   "Part":[{"id":3130,"key":"/library/parts/3130/1/file.mkv","container":"mkv","Stream":[
     {"id":31301,"streamType":1,"codec":"hevc","index":0,"bitrate":13900},
     {"id":31302,"streamType":2,"codec":"eac3","channels":6,"index":1,"languageCode":"eng","default":true,"selected":true}]}]},
  {"id":3087,"bitrate":8540,"width":1920,"height":800,"videoResolution":"1080","videoCodec":"h264","audioCodec":"aac","container":"mkv",
   "Part":[{"id":3135,"key":"/library/parts/3135/1/file.mkv","container":"mkv","Stream":[
     {"id":31351,"streamType":1,"codec":"h264","index":0,"bitrate":7900},
     {"id":31352,"streamType":2,"codec":"aac","channels":2,"index":1,"languageCode":"eng","default":true,"selected":true,"bitrate":640}]}]}]}]}}"#;

/// A loopback PMS that serves [`TWO_VERSIONS`] for the item, `mde` for every `/decision`, a
/// bounded body for `start.mkv`, and records every request line until it is told to stop.
fn versions_pms(mde: &'static [u8]) -> (i32, std::sync::mpsc::Sender<()>, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port() as i32;
    listener.set_nonblocking(true).unwrap();
    let (done, stop) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let mut requests = Vec::new();
        loop {
            match plx_base::testnet::accept(&listener) {
                Ok((mut socket, _)) => {
                    let line = drain_http(&mut socket);
                    let body: &[u8] = if line.starts_with("GET /library/metadata/rk-v") {
                        TWO_VERSIONS
                    } else if line.starts_with("GET /library/metadata/rk-4k") {
                        FOUR_K_FIRST
                    } else if line.starts_with("GET /library/metadata/rk-rung") {
                        FOUR_K_THEN_A_RUNG_SIZED_1080P
                    } else if line.contains("/decision?") {
                        mde
                    } else {
                        EMPTY_MC
                    };
                    write_json(&mut socket, body);
                    requests.push(line);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if stop.try_recv().is_ok() {
                        break;
                    }
                    std::thread::yield_now();
                }
                Err(e) => panic!("fixture accept: {e}"),
            }
        }
        requests
    });
    (port, done, handle)
}

/// Resolve `rk-v` as the request a menu's play makes: the default's part and codecs (what the
/// listing row carries), plus the version the viewer picked.
fn resolve(mde: &'static [u8], version: Option<plx_plex::plex::MediaId>) -> (Plan, Vec<String>) {
    resolve_with("rk-v", ("/library/parts/3134/1/file.mkv", "h264", "aac"), mde, |env| env.version = version)
}

/// [`resolve`] for any served item: `(part, vcodec, acodec)` is the default the caller names, and
/// `prepare` edits the environment the worker is handed.
fn resolve_with(
    rk: &str,
    (part, vcodec, acodec): (&str, &str, &str),
    mde: &'static [u8],
    prepare: impl FnOnce(&mut ResolveEnv),
) -> (Plan, Vec<String>) {
    let mut ps = crate::route::PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let (port, done, server) = versions_pms(mde);
    let sid = plx_plex::plex::register_for_test("versions", "127.0.0.1", port, "token", "versions-client");
    let mut env = ResolveEnv::snapshot(&ps, plx_data::stores::metadata::MetadataStore::default().view(), sid, rk);
    prepare(&mut env);
    let plan = build_stream(&plx_base::task::OffFrame::for_test(), rk, part, vcodec, acodec, &env);
    done.send(()).unwrap();
    let requests = server.join().unwrap();
    plx_plex::plex::reset_servers_for_test();
    (plan, requests)
}

const FOUR_K_DEFAULT: (&str, &str, &str) = ("/library/parts/3130/1/file.mkv", "hevc", "eac3");

/// A decoder that stops at 1080p HEVC: the 4K first copy cannot play directly on it.
fn full_hd_set() -> plx_platform::devcaps::Caps {
    plx_platform::devcaps::Caps { hevc_max: (1920, 1088), ..plx_platform::devcaps::Caps::assumed() }
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_1080p_decoder_plays_the_1080p_version_listed_after_the_4k() {
    let (plan, requests) = resolve_with("rk-4k", FOUR_K_DEFAULT, MDE_DIRECTPLAY, |env| {
        env.caps = std::sync::Arc::new(full_hd_set());
    });
    let mde = requests.iter().find(|l| l.contains("/decision?")).unwrap_or_else(|| panic!("{requests:?}"));
    assert_eq!(query_param(mde, "mediaIndex"), Some("1"), "{mde}");
    assert_eq!(query_param(mde, "audioStreamID"), Some("31342"), "{mde}");
    assert!(plan.url.contains("/library/parts/3134/"), "{}", plan.url);
    assert_eq!((plan.part_id, plan.media_index), (3134, 1));
    assert_eq!((plan.src_vcodec.as_str(), plan.src_acodec.as_str()), ("h264", "aac"));
    assert_eq!(plan.playing.as_ref().map(|p| (p.width, p.audio[0].id)), Some((1920, 31342)));
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_4k_decoder_plays_the_4k_version_listed_first_as_before() {
    let (plan, requests) = resolve_with("rk-4k", FOUR_K_DEFAULT, MDE_DIRECTPLAY, |_| {});
    let mde = requests.iter().find(|l| l.contains("/decision?")).unwrap_or_else(|| panic!("{requests:?}"));
    assert_eq!(query_param(mde, "mediaIndex"), Some("0"), "{mde}");
    assert_eq!((plan.part_id, plan.media_index), (3130, 0));
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_picked_version_wins_over_the_decoder() {
    let (plan, requests) = resolve_with("rk-4k", FOUR_K_DEFAULT, MDE_DIRECTPLAY, |env| {
        env.caps = std::sync::Arc::new(full_hd_set());
        env.version = Some(plx_plex::plex::MediaId(3082));
    });
    let mde = requests.iter().find(|l| l.contains("/decision?")).unwrap_or_else(|| panic!("{requests:?}"));
    assert_eq!(query_param(mde, "mediaIndex"), Some("0"), "{mde}");
    assert_eq!((plan.part_id, plan.media_index), (3130, 0));
}

#[test]
#[cfg(feature = "devtriggers")]
fn force_direct_play_attempts_the_4k_original_listed_first() {
    let (plan, _) = resolve_with("rk-4k", FOUR_K_DEFAULT, MDE_DIRECTPLAY, |env| {
        env.caps = std::sync::Arc::new(full_hd_set());
        env.direct_play_mode = DirectPlayMode::Forced;
    });
    assert_eq!((plan.part_id, plan.media_index), (3130, 0));
    assert!(plan.url.contains("/library/parts/3130/"), "{}", plan.url);
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_fixed_quality_measures_the_1080p_version_it_resolved() {
    let (plan, requests) = resolve_with("rk-4k", FOUR_K_DEFAULT, MDE_DIRECTPLAY, |env| {
        env.caps = std::sync::Arc::new(full_hd_set());
        env.quality = Quality::P1080;
    });
    assert_eq!(plan.src_measure, (2372, 1920, 800));
    assert!(plan.url.contains("/library/parts/3134/"), "{}", plan.url);
    assert!(!requests.iter().any(|l| l.contains("/transcode/universal/start")), "{requests:?}");
}

fn retry_holding(part_id: i64, audio_sid: i64, sub_sid: i64) -> RetryContext {
    RetryContext {
        direct_play_mode: DirectPlayMode::Auto,
        resume_ns: 0,
        selection: Some(HeldSelection { part_id, audio_sid, sub_sid }),
        sub_offset_ms: 0,
        suppress_enhancement: false,
        sub_mode: SubMode::Track,
    }
}

#[test]
fn a_retry_keeps_its_tracks_on_their_own_part_and_lets_another_part_choose_again() {
    let mut ps = crate::route::PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let meta = plx_data::stores::metadata::MetadataStore::default();
    let fresh_env = ResolveEnv::snapshot(&ps, meta.view(), ServerId::from_raw(0), "rk-4k");
    let fresh = (fresh_env.audio_sid, fresh_env.sub_sid, fresh_env.subtitle_override, fresh_env.sub_mode);

    let mut env = fresh_env.clone();
    apply_retry(&mut env, retry_holding(3130, 31302, 31303));
    assert!(env.with_selection_for_part(3130).is_none(), "the same part keeps the retry's ids");
    let moved = env.with_selection_for_part(3134).expect("another part chooses its own tracks");
    assert_eq!((moved.audio_sid, moved.sub_sid, moved.subtitle_override, moved.sub_mode), fresh);

    let mut unknown = fresh_env;
    apply_retry(&mut unknown, retry_holding(0, 31302, 31303));
    assert!(unknown.with_selection_for_part(3134).is_none(), "ids of an unknown part are kept as before");
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_retry_that_moves_to_the_1080p_version_does_not_carry_the_4k_tracks() {
    let retry = retry_holding(3130, 31302, 0);
    let (plan, requests) = resolve_with("rk-4k", FOUR_K_DEFAULT, MDE_TRANSCODE_VIDEO, |env| {
        env.caps = std::sync::Arc::new(full_hd_set());
        apply_retry(env, retry);
    });
    assert_eq!(plan.part_id, 3134);
    for line in requests.iter().chain(std::iter::once(&plan.url)) {
        assert!(!line.contains("31302"), "{line}");
    }
    assert!(
        requests.iter().any(|l| l.contains("/decision?") && query_param(l, "audioStreamID") == Some("31342")),
        "{requests:?}"
    );
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_version_picked_by_hand_is_measured_from_its_own_fetch() {
    let (plan, _) = resolve_with("rk-4k", FOUR_K_DEFAULT, MDE_DIRECTPLAY, |env| {
        env.caps = std::sync::Arc::new(full_hd_set());
        env.quality = Quality::P1080;
        env.version = Some(plx_plex::plex::MediaId(3086));
    });
    assert_eq!(plan.src_measure, (2372, 1920, 800));
    assert!(plan.url.contains("/library/parts/3134/"), "{}", plan.url);
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_picked_version_that_is_gone_is_measured_from_the_default_that_plays() {
    let (plan, _) = resolve_with("rk-4k", FOUR_K_DEFAULT, MDE_DIRECTPLAY, |env| {
        env.caps = std::sync::Arc::new(full_hd_set());
        env.quality = Quality::P1080;
        env.version = Some(plx_plex::plex::MediaId(9999));
    });
    assert_eq!(plan.src_measure.0, 14663);
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_fixed_quality_judges_the_substituted_version_on_its_video_bitrate() {
    let (plan, requests) = resolve_with("rk-rung", FOUR_K_DEFAULT, MDE_DIRECTPLAY, |env| {
        env.caps = std::sync::Arc::new(full_hd_set());
        env.quality = Quality::P1080;
    });
    assert_eq!(plan.src_measure, (7900, 1920, 800));
    assert!(plan.url.contains("/library/parts/3135/"), "{}", plan.url);
    assert!(!requests.iter().any(|l| l.contains("/transcode/universal/start")), "{requests:?}");
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_version_picked_by_hand_is_judged_on_its_video_bitrate() {
    let (plan, requests) = resolve_with("rk-rung", FOUR_K_DEFAULT, MDE_DIRECTPLAY, |env| {
        env.quality = Quality::P1080;
        env.version = Some(plx_plex::plex::MediaId(3087));
    });
    assert_eq!(plan.src_measure, (7900, 1920, 800));
    assert!(!requests.iter().any(|l| l.contains("/transcode/universal/start")), "{requests:?}");
}

/// A page whose default (`Media[0]`) the attempt's decoder cannot take must not hand the worker its
/// cached copy of that default: the worker fetches the item and chooses.
#[test]
fn a_page_whose_default_the_set_cannot_decode_gives_up_its_cached_copy() {
    let mut ps = crate::route::PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    struct Back;
    impl Drop for Back {
        fn drop(&mut self) { restore_direct_play_mode(DirectPlayMode::Auto); }
    }
    let _back = Back;
    let mut meta = plx_data::stores::metadata::MetadataStore::default();
    let sid = ServerId::from_raw(0);
    let version = |id, kbps, resolution: &str, width, height, vcodec: &str| plx_data::pms::MediaVersion {
        id: plx_plex::plex::MediaId(id),
        kbps,
        resolution: resolution.into(),
        width,
        height,
        vcodec: vcodec.into(),
        ..Default::default()
    };
    plx_data::metadata::set_current_for_test(
        meta.state_mut(),
        Some(plx_data::metadata::Detail {
            sid,
            rk: "rk-4k".into(),
            audio: vec![plx_data::metadata::Stream::default()],
            versions: vec![
                version(3082, 14663, "4k", 3840, 2160, "hevc"),
                version(3086, 2372, "1080", 1920, 800, "h264"),
            ],
            ..Default::default()
        }),
    );
    let request = PlaybackRequest {
        sid,
        rk: "rk-4k".into(),
        part: String::new(),
        vcodec: String::new(),
        acodec: String::new(),
        title: String::new(),
        ctx: String::new(),
        preview: false,
        version: None,
        show_rk: String::new(),
    };
    ps.begin_attempt(std::sync::Arc::new(plx_platform::devcaps::Caps::assumed()));
    assert!(request_env(&ps, meta.view(), &request, None).cached_item.is_some(), "a 4K decoder plays the page's own default");
    ps.begin_attempt(std::sync::Arc::new(full_hd_set()));
    restore_direct_play_mode(DirectPlayMode::Forced);
    assert!(request_env(&ps, meta.view(), &request, None).cached_item.is_some(), "Force Direct Play attempts the page's own default");
    restore_direct_play_mode(DirectPlayMode::Auto);
    assert!(request_env(&ps, meta.view(), &request, None).cached_item.is_none(), "the page describes the 4K, which this decoder cannot take");
    let picked = PlaybackRequest { version: Some(plx_plex::plex::MediaId(3082)), ..request };
    assert!(request_env(&ps, meta.view(), &picked, None).cached_item.is_none(), "a pick never uses the page's copy");
    plx_data::metadata::set_current_for_test(meta.state_mut(), None);
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_picked_version_direct_plays_its_own_part_after_asking_about_it() {
    let (plan, requests) = resolve(MDE_DIRECTPLAY, Some(plx_plex::plex::MediaId(3082)));
    let mde = requests.iter().find(|l| l.contains("/decision?")).unwrap_or_else(|| panic!("{requests:?}"));
    assert_eq!(query_param(mde, "mediaIndex"), Some("1"), "the MDE asks about the picked version: {mde}");
    assert_eq!(query_param(mde, "audioStreamID"), Some("31302"), "with that version's audio: {mde}");
    assert!(plan.url.contains("/library/parts/3130/"), "the picked version's part plays: {}", plan.url);
    assert_eq!((plan.part_id, plan.media_index), (3130, 1));
    assert_eq!((plan.src_vcodec.as_str(), plan.src_acodec.as_str()), ("hevc", "eac3"));
    let playing = plan.playing.as_ref().expect("track store");
    assert_eq!((playing.width, playing.audio[0].id), (3840, 31302), "the track store is the picked version's");
    for put in requests.iter().filter(|l| l.starts_with("PUT /library/parts/")) {
        assert!(put.starts_with("PUT /library/parts/3130"), "a selection names the picked part: {put}");
    }
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_picked_version_transcodes_from_its_own_index() {
    let (plan, requests) = resolve(MDE_TRANSCODE_VIDEO, Some(plx_plex::plex::MediaId(3082)));
    let decisions: Vec<&String> = requests.iter().filter(|l| l.contains("/decision?")).collect();
    assert!(decisions.len() >= 2, "an MDE ask and a transcode decision: {requests:?}");
    for line in &decisions {
        assert_eq!(query_param(line, "mediaIndex"), Some("1"), "{line}");
    }
    assert!(plan.url.contains("start.mkv"), "{}", plan.url);
    assert!(plan.url.contains("mediaIndex=1&"), "the start URL names the picked version: {}", plan.url);
    assert_eq!(plan.part_id, 3130);
    for put in requests.iter().filter(|l| l.starts_with("PUT /library/parts/")) {
        assert!(put.starts_with("PUT /library/parts/3130"), "a selection names the picked part: {put}");
    }
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_version_the_item_no_longer_lists_plays_the_default() {
    let (plan, requests) = resolve(MDE_DIRECTPLAY, Some(plx_plex::plex::MediaId(9)));
    let mde = requests.iter().find(|l| l.contains("/decision?")).unwrap_or_else(|| panic!("{requests:?}"));
    assert_eq!(query_param(mde, "mediaIndex"), Some("0"), "{mde}");
    assert!(plan.url.contains("/library/parts/3134/"), "{}", plan.url);
    assert_eq!((plan.part_id, plan.media_index), (3134, 0));
    assert_eq!(plan.playing.as_ref().map(|p| p.audio[0].id), Some(31342));
}

#[test]
#[cfg(feature = "devtriggers")]
fn no_pick_is_the_default_version_exactly_as_before() {
    let (plan, requests) = resolve(MDE_DIRECTPLAY, None);
    let mde = requests.iter().find(|l| l.contains("/decision?")).unwrap_or_else(|| panic!("{requests:?}"));
    assert_eq!(query_param(mde, "mediaIndex"), Some("0"), "{mde}");
    assert_eq!((plan.part_id, plan.media_index), (3134, 0));
}


/// A retry or quality change resolves the same item again from the stored request, so the pick
/// rides it; the session installs the landed version for every later encoder of the playback.
#[test]
fn the_picked_version_rides_the_request_and_the_landing() {
    let mut ps = crate::route::PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let mut meta = plx_data::stores::metadata::MetadataStore::default();
    assert!(request_play(&mut ps, &mut meta, plx_platform::devcaps::caps(), ServerId::from_raw(0), "rk-v", "/library/parts/3134/1/file.mkv",
        "h264", "aac", "Episode", "", Some(plx_plex::plex::MediaId(3082)), ""));
    assert_eq!(ps.request.as_ref().and_then(|r| r.version), Some(plx_plex::plex::MediaId(3082)));
    let env = request_env(&ps, meta.view(), ps.request.as_ref().unwrap(), None);
    assert_eq!(env.version, Some(plx_plex::plex::MediaId(3082)));
    assert!(env.cached_item.is_none() && env.src_kbps == 0, "the default's facts never stand in for the pick");
    cancel_play(&mut ps);
    test_support::apply_plan(&mut ps, Plan { media_index: 1, ..Default::default() }, "rk-v");
    assert_eq!(cur_media_index(&ps), 1);
}
