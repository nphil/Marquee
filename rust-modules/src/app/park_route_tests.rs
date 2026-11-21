//! **The background park, as the loop takes it** (`app::park`, `run::run`): Home over a hero
//! carousel, ten seconds in the foreground, ten in the background, ten back. The loop is driven on
//! a fake clock the way `run` drives it: each iteration asks [`Park::turn`]; an ordinary frame
//! costs the 16 ms idle poll, a parked turn its whole wait.
//!
//! Measured on the TV before the park (2026-11-20): a backgrounded Home woke the loop 60 times a
//! second and turned the hero 55 times, each turn a backdrop fetched and decoded.

use super::test_support::World;
use crate::app::park::{Park, Turn, PARK_WAIT_MS};

/// What ten seconds of the loop did.
struct Span {
    wakes: u32,
    frames: u32,
    heroes: Vec<String>,
}

fn hero(w: &World) -> String {
    let home: &plx_screens::home::HomeScreen = w.d.top_screen().and_then(|s| s.as_any())
        .and_then(|any| any.downcast_ref()).expect("Home is on top");
    home.hero_for_test().expect("the carousel shows a hero").to_owned()
}

fn ten_seconds(w: &mut World, park: &mut Park, background: bool, turns: &mut Vec<Turn>) -> Span {
    let (mut clock, frame0) = (0, w.frame);
    let mut span = Span { wakes: 0, frames: 0, heroes: vec![hero(w)] };
    while clock < 10_000 {
        span.wakes += 1;
        let turn = park.turn(background);
        if turns.last() != Some(&turn) { turns.push(turn); }
        if matches!(turn, Turn::Enter | Turn::Park) {
            if turn == Turn::Enter { crate::app::park::release(); }
            clock += PARK_WAIT_MS;
            continue;
        }
        w.step(vec![]);
        clock += 16;
        let now = hero(w);
        if span.heroes.last() != Some(&now) { span.heroes.push(now); }
    }
    span.frames = w.frame - frame0;
    span
}

#[test]
fn a_backgrounded_home_waits_and_turns_no_hero_until_the_foreground() {
    let mut w = World::home_heroes("park", 3);
    let (mut park, mut turns) = (Park::default(), Vec::new());

    let before = ten_seconds(&mut w, &mut park, false, &mut turns);
    assert!(before.wakes >= 600, "premise: the foreground loop polls at 60 Hz ({} wakes)", before.wakes);
    assert!(before.heroes.len() > 1, "premise: the hero turns in ten seconds ({:?})", before.heroes);

    let parked = ten_seconds(&mut w, &mut park, true, &mut turns);
    assert!(parked.wakes <= 10, "the parked loop waits instead of polling ({} wakes)", parked.wakes);
    assert_eq!(parked.frames, 0, "no frame, so no page tick and no art requested or drawn");
    assert_eq!(parked.heroes.len(), 1, "the hero holds still: {:?}", parked.heroes);

    let after = ten_seconds(&mut w, &mut park, false, &mut turns);
    assert!(after.wakes >= 600 && after.frames == after.wakes, "frames resume at the foreground cadence");
    assert!(after.heroes.len() > 1, "and the carousel turns again ({:?})", after.heroes);
    let words: Vec<String> = w.drawn().words.into_iter().map(|(s, _)| s).collect();
    assert!(["Home", "Play"].iter().all(|s| words.iter().any(|w| w == s)), "Home and its hero draw after the release: {words:?}");
    assert_eq!(turns, [Turn::Frame, Turn::Enter, Turn::Park, Turn::Resume, Turn::Frame],
        "each edge is one turn, where the loop logs it");
}
