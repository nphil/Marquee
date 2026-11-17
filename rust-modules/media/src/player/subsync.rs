//! **Automatic sync of a downloaded subtitle.** A subtitle file fetched beside the video (a
//! sidecar, often from Bazarr) is often timed for another release of the film and runs a constant
//! few seconds off. When one is picked and the video carries a text subtitle track of its own, the
//! two are compared for ONE constant shift ([`find_shift`]): in the same language the words both
//! say vote for it, otherwise where the lines start, end and overlap decides. Only a clear answer
//! is applied, and only a shift of half a second or more.
//!
//! The shift moves the sidecar's clock (`sidecar::sync_offset_ms`, read by
//! `player::sidecar_clock_ns`); the file's own cue times never change, so a later comparison still
//! starts from them. The viewer's Timing offset composes on top. Nothing is compared while the
//! viewer has an offset of their own, or while SpeechDock aligns the words of the subtitle to the
//! audio (`pump::aligning`).
//!
//! [`update`] runs on the frame thread and only decides that a comparison is due; the comparison
//! runs on a worker over copies of the two cue lists.

use std::cmp::Ordering::{Equal, Greater, Less};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};

use super::sidecar::SyncTarget;

/// A shift further than this is a different cut of the film, not a delay, and is not searched.
const RANGE_MS: i64 = 30_000;
/// A shift smaller than this is left alone: nobody notices it, and the file may well be right.
const MIN_SHIFT_MS: i64 = 500;
/// Two answers this far apart put the subtitle in clearly different places.
const APART_MS: i64 = 1_500;
/// The timing comparison's grid.
const STEP_MS: i64 = 50;
/// The coarse scan's step, whose best peaks are then refined on [`STEP_MS`].
const COARSE_MS: i64 = 200;
/// A sidecar line starts (or ends) with one of the file's when the two are this close.
const NEAR_MS: i64 = 350;
/// How much of the shifted sidecar must fall inside the span of the file's cues to be scored.
const MIN_OVERLAP_MS: i64 = 20_000;
/// Votes for shifts this close together are one answer.
const CLUSTER_MS: i64 = 600;

/// The first comparison waits this long after the file is shown: by then the demuxer has read the
/// video's own track around the playhead.
const FIRST_LOOK_MS: u32 = 10_000;
/// An unsure comparison is made again this much later, with more of the video's track read…
const RETRY_MS: u32 = 20_000;
/// …this many times at most.
const LOOKS: u8 = 6;

/// One cue as the matcher reads it: on screen from `start_ms` to `end_ms`, saying `text`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Cue<'a> {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: &'a str,
}

/// The answer: shown `ms` later (earlier when negative), the sidecar lines up with the video's own
/// track. `by_words` when the words both say decided it, else the timing alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Shift {
    pub ms: i64,
    pub by_words: bool,
}

/// **The one constant shift that lines `sidecar` up with `own`, the video's own track**, or `None`
/// when no shift is clearly right. `same_language` lets the words decide first.
pub(super) fn find_shift(sidecar: &[Cue<'_>], own: &[Cue<'_>], same_language: bool) -> Option<Shift> {
    let own = lines(own.iter(), same_language);
    let from = own.iter().map(|l| l.start).min()?;
    let to = own.iter().map(|l| l.end).max()?;
    // Only the stretch of the sidecar that a shift in range can lay over the video's cues.
    let reach = sidecar.iter().filter(|c| c.end_ms > from - RANGE_MS && c.start_ms < to + RANGE_MS);
    let sidecar = lines(reach, same_language);
    if sidecar.len() < 10 || own.len() < 6 {
        return None;
    }
    if same_language {
        if let Some(ms) = by_words(&sidecar, &own) {
            return Some(Shift { ms: (ms as f64 / 10.0).round() as i64 * 10, by_words: true });
        }
    }
    by_timing(&sidecar, &own).map(|ms| Shift { ms, by_words: false })
}

/// A cue as compared: its times and, when words count, its distinct words (hashed, sorted) and how
/// many words it says.
struct Line {
    start: i64,
    end: i64,
    words: Vec<u32>,
    said: usize,
}

/// The cues worth comparing, in start order: a cue with no length shows nothing.
fn lines<'a>(cues: impl Iterator<Item = &'a Cue<'a>>, words: bool) -> Vec<Line> {
    let mut lines: Vec<Line> = cues
        .filter(|c| c.end_ms > c.start_ms)
        .map(|c| {
            let (words, said) = if words { words_of(c.text) } else { (Vec::new(), 0) };
            Line { start: c.start_ms, end: c.end_ms, words, said }
        })
        .collect();
    lines.sort_by_key(|l| l.start);
    lines
}

/// The words of a line, for comparing two lines in one language: runs of letters and digits of
/// any script, lower-cased, of two characters or more, at most 64; markup (`<i>`, `{\an8}`) is
/// not words. Returned hashed (FNV-1a), sorted and without repeats, with the count before repeats
/// went.
fn words_of(text: &str) -> (Vec<u32>, usize) {
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut words = Vec::new();
    let (mut hash, mut letters, mut markup) = (FNV_OFFSET, 0usize, None);
    for c in text.chars().chain([' ']) {
        if let Some(close) = markup {
            if c == close {
                markup = None;
            }
            continue;
        }
        markup = match c {
            '<' => Some('>'),
            '{' => Some('}'),
            _ => None,
        };
        if markup.is_none() && c.is_alphanumeric() {
            for lower in c.to_lowercase() {
                hash = (hash ^ u32::from(lower)).wrapping_mul(FNV_PRIME);
            }
            letters += 1;
            continue;
        }
        if letters > 1 && words.len() < 64 {
            words.push(hash);
        }
        (hash, letters) = (FNV_OFFSET, 0);
    }
    let said = words.len();
    words.sort_unstable();
    words.dedup();
    (words, said)
}

/// How many words two sorted lists without repeats share.
fn shared(a: &[u32], b: &[u32]) -> usize {
    let (mut i, mut j, mut n) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Less => i += 1,
            Greater => j += 1,
            Equal => {
                n += 1;
                i += 1;
                j += 1;
            }
        }
    }
    n
}

/// **The shift the shared words vote for.** Every line of the video's own track that says three
/// words or more is paired with each sidecar line within [`RANGE_MS`] that says nearly the same
/// (60% of their distinct words in common, two at least); each pair votes for the shift that lines
/// the two up. Votes within [`CLUSTER_MS`] of each other are one answer and its median the shift.
/// It stands with four votes or more and two and a half times the votes of any answer
/// [`APART_MS`] away. Needs eight lines with words on the video's side.
fn by_words(sidecar: &[Line], own: &[Line]) -> Option<i64> {
    if own.iter().filter(|l| !l.words.is_empty()).count() < 8 {
        return None;
    }
    let mut votes = Vec::new();
    for line in own.iter().filter(|l| l.said >= 3) {
        let first = sidecar.partition_point(|s| s.start < line.start - RANGE_MS);
        for s in sidecar[first..].iter().take_while(|s| s.start <= line.start + RANGE_MS) {
            if s.said < 2 {
                continue;
            }
            let common = shared(&s.words, &line.words);
            let union = s.words.len() + line.words.len() - common;
            if common >= 2 && common * 5 >= union * 3 {
                votes.push(line.start - s.start);
            }
        }
    }
    votes.sort_unstable();
    // Each vote's answer: the votes from the earliest within CLUSTER_MS of it up to it.
    let mut first = 0;
    let answers: Vec<(usize, i64)> = (0..votes.len())
        .map(|last| {
            while votes[last] - votes[first] > CLUSTER_MS {
                first += 1;
            }
            let n = last - first + 1;
            (n, votes[first + n / 2])
        })
        .collect();
    let &(n, at) = answers.iter().max_by_key(|(n, _)| *n)?;
    let rival = answers.iter().filter(|(_, d)| (d - at).abs() > APART_MS).map(|(n, _)| *n).max().unwrap_or(0);
    (n >= 4 && n * 2 >= rival.max(1) * 5).then_some(at)
}

/// **The shift where the lines fall**, for a sidecar in another language or one whose words did
/// not decide. Every shift within [`RANGE_MS`], [`COARSE_MS`] apart, is scored by how much of the
/// shifted sidecar falls on the video's cues; the six best peaks are refined on the [`STEP_MS`]
/// grid, where the score adds how many sidecar lines start and end within [`NEAR_MS`] of one of
/// the video's (weighted 0.65, the overlap 0.35). The best stands at 0.45 or more and 0.08 above
/// anything [`APART_MS`] away: evenly spaced lines fit several shifts alike and are refused.
fn by_timing(sidecar: &[Line], own: &[Line]) -> Option<i64> {
    let grid = Grid::of(own)?;
    let spans: Vec<(i64, i64)> = sidecar
        .iter()
        .filter(|l| l.end > grid.from - RANGE_MS && l.start < grid.to + RANGE_MS)
        .map(|l| (l.start, l.end))
        .collect();
    if spans.len() < 6 {
        return None;
    }
    let shift_at = |k: usize| -RANGE_MS + k as i64 * COARSE_MS;
    let coarse: Vec<Option<f64>> = (0..=(2 * RANGE_MS / COARSE_MS) as usize)
        .map(|k| grid.tally(&spans, shift_at(k), false).map(|t| t.overlap()))
        .collect();
    let mut peaks: Vec<usize> = (0..coarse.len()).filter(|&k| is_peak(&coarse, k)).collect();
    peaks.sort_by(|a, b| coarse[*b].partial_cmp(&coarse[*a]).unwrap_or(Equal));
    peaks.truncate(6);
    let mut scored = Vec::new();
    for k in peaks {
        let center = shift_at(k);
        for d in (center - 2 * COARSE_MS..=center + 2 * COARSE_MS).step_by(STEP_MS as usize) {
            if let Some(t) = grid.tally(&spans, d, true) {
                scored.push((t.score(), d));
            }
        }
    }
    let &(best, at) = scored.iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
    let rival = scored.iter().filter(|(_, d)| (d - at).abs() > APART_MS).map(|(s, _)| *s).fold(0.0, f64::max);
    let hundredths = |x: f64| (x * 100.0).round();
    (hundredths(best) >= 45.0 && hundredths(best - rival) >= 8.0).then_some(at)
}

/// Is `scan[k]` a peak: above its three neighbours on each side (the earliest of a level run)?
fn is_peak(scan: &[Option<f64>], k: usize) -> bool {
    let Some(v) = scan[k] else { return false };
    let near = k.saturating_sub(3)..=(k + 3).min(scan.len() - 1);
    near.into_iter().all(|q| match scan[q] {
        _ if q == k => true,
        Some(w) => w < v || (w == v && q > k),
        None => true,
    })
}

/// The video's cues as the timing comparison reads them: which [`STEP_MS`] cells of their span a
/// cue covers (as a running count, so any stretch is two lookups), and their starts and ends.
struct Grid {
    from: i64,
    to: i64,
    covered: Vec<u32>,
    starts: Vec<i64>,
    ends: Vec<i64>,
}

/// One shift's tally: time of the shifted sidecar on the video's cues and inside their span, and
/// how many of its line starts and ends meet one of the video's.
struct Tally {
    covered: i64,
    inside: i64,
    met: usize,
    edges: usize,
}

impl Tally {
    fn overlap(&self) -> f64 {
        self.covered as f64 / self.inside as f64
    }

    fn score(&self) -> f64 {
        0.35 * self.overlap() + 0.65 * self.met as f64 / self.edges.max(1) as f64
    }
}

impl Grid {
    /// `None` for fewer than six cues or a span over four hours.
    fn of(own: &[Line]) -> Option<Grid> {
        let from = own.iter().map(|l| l.start).min()?;
        let to = own.iter().map(|l| l.end).max()?;
        if own.len() < 6 || to - from > 4 * 3_600_000 {
            return None;
        }
        let cells = ((to - from) / STEP_MS) as usize + 1;
        let mut on = vec![false; cells];
        for l in own {
            on[((l.start - from) / STEP_MS) as usize..=((l.end - from) / STEP_MS) as usize].fill(true);
        }
        let mut covered = Vec::with_capacity(cells + 1);
        covered.push(0);
        for (i, cell) in on.iter().enumerate() {
            covered.push(covered[i] + u32::from(*cell));
        }
        let mut starts: Vec<i64> = own.iter().map(|l| l.start).collect();
        let mut ends: Vec<i64> = own.iter().map(|l| l.end).collect();
        starts.sort_unstable();
        ends.sort_unstable();
        Some(Grid { from, to, covered, starts, ends })
    }

    /// The time from `a` to `b` the video's cues cover, to the grid.
    fn cover(&self, a: i64, b: i64) -> i64 {
        let last = self.covered.len() - 1;
        let cell = |t: i64| (((t - self.from).max(0) / STEP_MS) as usize).min(last);
        let (a, b) = (cell(a), cell(b));
        if b <= a { 0 } else { i64::from(self.covered[b] - self.covered[a]) * STEP_MS }
    }

    /// The sidecar shifted by `d`, counted against the video's cues; `edges` adds where lines start
    /// and end. `None` when too little of it falls inside the span to judge.
    fn tally(&self, spans: &[(i64, i64)], d: i64, edges: bool) -> Option<Tally> {
        let mut t = Tally { covered: 0, inside: 0, met: 0, edges: 0 };
        for &(start, end) in spans {
            let (start, end) = (start + d, end + d);
            if end <= self.from || start >= self.to {
                continue;
            }
            t.inside += end.min(self.to) - start.max(self.from);
            t.covered += self.cover(start, end);
            if edges {
                t.edges += 2;
                t.met += usize::from(near(&self.starts, start)) + usize::from(near(&self.ends, end));
            }
        }
        (t.inside >= MIN_OVERLAP_MS).then_some(t)
    }
}

/// Is any of `sorted` within [`NEAR_MS`] of `t`?
fn near(sorted: &[i64], t: i64) -> bool {
    let i = sorted.partition_point(|v| *v < t);
    sorted.get(i).is_some_and(|v| v - t < NEAR_MS) || (i > 0 && t - sorted[i - 1] < NEAR_MS)
}

/// The sync's progress for the sidecar on screen.
struct Job {
    /// The file the looks are for.
    target: Option<SyncTarget>,
    /// The video's own cues read so far, in the sidecar's (file) time: each look adds what the
    /// demuxer holds now, so a later look compares more.
    own: BTreeMap<(i64, i64), Arc<str>>,
    /// The video track `own` was read from.
    track: i32,
    looks: u8,
    /// The tick a look is next due at.
    due: u32,
    /// A look is running on a worker.
    running: bool,
    /// Synced, in sync already, or given up: nothing more for this file.
    settled: bool,
}

static JOB: Mutex<Job> = Mutex::new(Job {
    target: None,
    own: BTreeMap::new(),
    track: -1,
    looks: 0,
    due: 0,
    running: false,
    settled: false,
});

fn lock() -> MutexGuard<'static, Job> {
    JOB.lock().unwrap_or_else(|e| e.into_inner())
}

/// A new item is starting: the previous one's sync is over, and a look still running for it is
/// dropped when it lands.
pub(super) fn reset() {
    let mut job = lock();
    job.target = None;
    job.own = BTreeMap::new();
}

/// **Once a frame, from `pump`: start a look when one is due.** Twice a second at most, and only
/// lock reads unless a look starts.
pub(super) fn update(ps: &crate::route::PlaybackSession, now: u32) {
    static LAST: AtomicU32 = AtomicU32::new(0);
    if now.wrapping_sub(LAST.load(Relaxed)) < 500 {
        return;
    }
    LAST.store(now, Relaxed);
    let mut job = lock();
    if !job.target.as_ref().is_some_and(super::sidecar::is_sync_target) {
        // Off, or an embedded track, keeps the job: picking the same file again resumes it.
        let Some(target) = super::sidecar::sync_target() else { return };
        *job = Job {
            target: Some(target),
            own: BTreeMap::new(),
            track: -1,
            looks: 0,
            due: now.wrapping_add(FIRST_LOOK_MS),
            running: false,
            settled: false,
        };
    }
    if job.settled || job.running || (now.wrapping_sub(job.due) as i32) < 0 {
        return;
    }
    if super::subtitle_offset_ms() != 0 || super::pump::aligning() {
        job.settled = true;
        return;
    }
    if crate::route::subtitles_burned(ps)
        || super::loading(ps)
        || !super::is_playing(ps)
        || super::frames() <= 0
        || crate::route::subtitle_activation(ps, crate::route::cur_sub_sid(ps)) != crate::route::SubtitleActivation::Active
    {
        return;
    }
    // The video's cues run on the playback clock and a sidecar on the file's; over a remux the two
    // differ by the side reader's delta, and until it is measured nothing is compared.
    let Some(delta_ns) = super::sidecar_delta_ns(ps) else { return };
    let language = crate::route::alignment_playback_identity(ps, None).and_then(|p| p.subtitle_language).unwrap_or("");
    let same = |track: i32| {
        !language.is_empty()
            && crate::route::embedded_subtitle_language(ps, track).is_some_and(|l| plx_data::metadata::lang_matches(l, language))
    };
    job.looks += 1;
    job.due = now.wrapping_add(RETRY_MS);
    // The video's track to compare with: one in the sidecar's language first, then the fullest.
    let Some((same_language, _, track)) =
        super::ass_source::text_tracks().into_iter().map(|(track, held)| (same(track), held, track)).max()
    else {
        job.settled = job.looks >= LOOKS;
        return;
    };
    let Some(target) = job.target.clone() else { return };
    job.running = true;
    drop(job);
    let delta_ms = delta_ns / 1_000_000;
    if !plx_base::task::spawn_small("subtitle sync", move || look(target, track, same_language, delta_ms)) {
        let mut job = lock();
        job.running = false;
        job.settled = true;
    }
}

/// One look, on a worker: compare, and apply a clear answer to the file it was made for.
fn look(target: SyncTarget, track: i32, same_language: bool, delta_ms: i64) {
    let fresh = super::ass_source::text_cues(track);
    let own: Vec<(i64, i64, Arc<str>)> = {
        let mut job = lock();
        if job.target.as_ref() != Some(&target) {
            return;
        }
        if job.track != track {
            job.own.clear();
            job.track = track;
        }
        for (start, end, text) in fresh {
            job.own.insert((start + delta_ms, end + delta_ms), text);
        }
        job.own.iter().map(|((start, end), text)| (*start, *end, text.clone())).collect()
    };
    let shift = super::sidecar::sync_script(&target).and_then(|script| {
        let sidecar: Vec<Cue<'_>> = script
            .source_cues
            .iter()
            .map(|c| Cue { start_ms: c.authored_start_ms, end_ms: c.authored_end_ms, text: &c.text })
            .collect();
        let own: Vec<Cue<'_>> = own.iter().map(|(start, end, text)| Cue { start_ms: *start, end_ms: *end, text }).collect();
        find_shift(&sidecar, &own, same_language)
    });
    let mut job = lock();
    if job.target.as_ref() != Some(&target) {
        return;
    }
    job.running = false;
    let Some(shift) = shift else {
        if job.looks >= LOOKS {
            job.settled = true;
            super::log("subsync: no clear match; the sidecar keeps its timing");
        }
        return;
    };
    job.settled = true;
    drop(job);
    let how = if shift.by_words { "words" } else { "timing" };
    if shift.ms.abs() < MIN_SHIFT_MS {
        super::log(&format!("subsync: in sync already ({} ms by {how})", shift.ms));
        return;
    }
    // The viewer may have set a timing, or SpeechDock started aligning, while this ran.
    if super::subtitle_offset_ms() != 0 || super::pump::aligning() || !super::sidecar::apply_sync(&target, shift.ms) {
        return;
    }
    super::log(&format!("subsync: sidecar moved {} ms by {how}", shift.ms));
    plx_machine::present::wake_from_worker();
    let _ = plx_platform::tv::toast::toast(&notice(shift.ms));
}

/// The toast: "Subtitles synced, 2.4 s earlier".
fn notice(ms: i64) -> String {
    use plx_platform::i18n::msg;
    let locale = plx_platform::i18n::current();
    let tenths = ((ms.unsigned_abs() + 50) / 100) as i64;
    let seconds = msg::core_seconds_in(locale, &locale.decimal(tenths, 1));
    if ms < 0 {
        msg::widgets_tracks_synced_earlier_in(locale, &seconds)
    } else {
        msg::widgets_tracks_synced_later_in(locale, &seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Irregular dialogue timing from `seed`: `n` cues of 0.9–3.9 s with 0.4–4.4 s gaps, from 60 s.
    fn dialogue(n: usize, seed: u64) -> Vec<(i64, i64)> {
        let mut x = seed;
        let mut next = move |m: i64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % m as u64) as i64
        };
        let mut t = 60_000;
        (0..n)
            .map(|_| {
                t += 400 + next(4_000);
                let d = 900 + next(3_000);
                t += d;
                (t - d, t)
            })
            .collect()
    }

    fn cues<'a>(times: &[(i64, i64)], texts: &'a [String]) -> Vec<Cue<'a>> {
        times.iter().zip(texts).map(|(&(start_ms, end_ms), text)| Cue { start_ms, end_ms, text }).collect()
    }

    const SAID: [&str; 24] = [
        "Where were you last night?", "I told you, I was at the office.", "Nobody saw you there, Michael.",
        "Then nobody was looking hard enough.", "Don't lie to me, not tonight.", "Fine. I went to see my brother.",
        "Your brother has been gone for years.", "He came back last week, he needs money.", "How much money are we talking about?",
        "More than we have in the bank.", "Then he can find it somewhere else.", "He's family, we can't just leave him.",
        "Watch me close the door on him.", "You always do this when it gets hard.", "Somebody in this house has to be sensible.",
        "The car is outside, are you coming?", "Give me five minutes to find my coat.", "We are already twenty minutes late.",
        "Then two more won't make any difference.", "Did you remember to lock the back door?", "I locked it twice before dinner.",
        "Good, the neighbours were burgled again.", "That's the third time this summer.", "Let's go before I change my mind.",
    ];

    #[test]
    fn the_same_words_find_a_sidecar_running_late() {
        let times = dialogue(24, 7);
        let own_text: Vec<String> = SAID.iter().map(|s| s.to_string()).collect();
        // The sidecar: every line 2.4 s late (±40 ms), in markup, with captions the video's track lacks.
        let mut side_times: Vec<(i64, i64)> =
            times.iter().enumerate().map(|(i, &(s, e))| (s + 2_400 + (i as i64 % 5 - 2) * 20, e + 2_400)).collect();
        let mut side_text: Vec<String> = SAID.iter().map(|s| format!("<i>{s}</i>")).collect();
        side_text[3] = format!("{{\\an8}}{}", SAID[3]);
        for (k, at) in [(0, 61_000), (1, 95_000), (2, 130_000)] {
            side_times.push((at, at + 1_500));
            side_text.push(["[door slams]", "(MUSIC PLAYING)", "[thunder]"][k].to_owned());
        }
        // The video's track has two lines the sidecar dropped.
        let own = cues(&times[2..], &own_text[2..]);
        let shift = find_shift(&cues(&side_times, &side_text), &own, true).expect("a clear match");
        assert!(shift.by_words, "{shift:?}");
        assert!((shift.ms + 2_400).abs() <= 40, "{shift:?}");
    }

    #[test]
    fn timing_alone_finds_a_sidecar_in_another_language_running_early() {
        let times = dialogue(36, 11);
        let own_text: Vec<String> = (0..36).map(|i| format!("line {i}")).collect();
        let side_text: Vec<String> = (0..36).map(|i| format!("línea número {i}")).collect();
        // 3.1 s early, each end ragged by up to 80 ms the way another subtitler times a line.
        let side_times: Vec<(i64, i64)> = times
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| (s - 3_100 + (i as i64 % 3 - 1) * 80, e - 3_100 - (i as i64 % 4) * 25))
            .collect();
        let shift = find_shift(&cues(&side_times, &side_text), &cues(&times, &own_text), false).expect("a clear match");
        assert!(!shift.by_words, "{shift:?}");
        assert!((shift.ms - 3_100).abs() <= 100, "{shift:?}");
    }

    #[test]
    fn evenly_spaced_or_unrelated_lines_are_left_alone() {
        let text: Vec<String> = (0..40).map(|i| format!("{i}")).collect();
        let even = |offset: i64| -> Vec<(i64, i64)> { (0..40).map(|i| (60_000 + offset + i * 3_000, 61_500 + offset + i * 3_000)).collect() };
        let (own, side) = (even(0), even(1_200));
        assert_eq!(find_shift(&cues(&side, &text), &cues(&own, &text), false), None, "every 3 s fits alike");
        let (own, side) = (dialogue(30, 3), dialogue(30, 1_234));
        assert_eq!(find_shift(&cues(&side, &text), &cues(&own, &text), false), None, "nothing lines up");
    }
}
