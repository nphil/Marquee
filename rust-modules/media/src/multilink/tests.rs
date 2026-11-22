//! Multi Link against a made-up network ([`super::fakenet`]): a clock the test moves, no socket
//! and no sleep.

use std::net::Ipv4Addr;

use super::engine::{Event, Micros, Phase, Request};
use super::fakenet::*;
use super::status::{self, Candidate, Kind, Unusable};

/// What a proof asks for; every range a link carries for the film is longer, or ends the file.
const PROOF: u64 = 64 << 10;

fn wire() -> Candidate {
    candidate("eth0", Kind::Ethernet, WIRE)
}

fn wifi() -> Candidate {
    candidate("wlan0", Kind::Wifi, WIFI)
}

fn usb() -> Candidate {
    candidate("eth1", Kind::UsbEthernet, USB)
}

/// The owner's television: a 100 Mbit/s wire that carries 94, and Wi-Fi that carries 180.
fn wire_and_wifi(len: usize) -> Sim {
    Sim::new(len, vec![Path::new(WIRE, mbit(94)), Path::new(WIFI, mbit(180))], vec![wire(), wifi()])
}

fn asked(request: &Request) -> u64 {
    request.last - request.first + 1
}

/// Bits a second of `bytes` over `over`.
fn bps(bytes: u64, over: Micros) -> u64 {
    bytes * 8 * 1_000_000 / over
}

/// The demuxer read the whole file, once, in order.
fn assert_whole(sim: &Sim) {
    assert_eq!(sim.read.len(), 1, "one run from the head");
    assert_eq!(sim.read[0].1.len() as u64, sim.total(), "every byte was read");
    sim.assert_exact();
}

/// No byte of the film came twice: a range that changed links was asked from where it stood.
fn assert_no_byte_twice(sim: &Sim) {
    let mut spans: Vec<(u64, u64)> = sim.net.spans().iter().filter(|(r, n)| asked(r) != PROOF && *n > 0).map(|(r, n)| (r.first, r.first + n)).collect();
    spans.sort_unstable();
    assert!(spans.windows(2).all(|pair| pair[0].1 <= pair[1].0), "a byte was delivered twice\n{}", sim.logs.join("\n"));
}

/// The first thing asked from `source` after `after` is a proof.
fn assert_proven_anew(sim: &Sim, source: Ipv4Addr, after: Micros) {
    let first = sim.net.started.iter().find(|(at, r, _)| *at > after && r.source == source).map(|(_, r, _)| asked(r));
    assert_eq!(first, Some(PROOF), "{source} carries again only after a new proof\n{}", sim.logs.join("\n"));
}

/// Run until both links carry, and return when that was.
fn carrying_both(sim: &mut Sim) -> Micros {
    assert!(sim.run_until(3_000, |s| s.logged("using wlan0") && s.net.live().len() == 2), "both links carry\n{}", sim.logs.join("\n"));
    sim.now
}

#[test]
fn two_links_join_ranges_that_arrive_out_of_order_into_the_exact_file() {
    let mut sim = wire_and_wifi(16 << 20);
    sim.finish(4_000);
    assert_whole(&sim);

    let started = sim.logged_at("carrying from byte").expect("the links took the stream over") * MS;
    let (from, to) = (started + 200 * MS, started + 500 * MS);
    let (wire, wifi) = (sim.net.bytes(WIRE, from, to), sim.net.bytes(WIFI, from, to));
    assert!(bps(wire + wifi, to - from) > 200_000_000, "together they carry more than either alone: {}", bps(wire + wifi, to - from));
    assert!(bps(wifi, to - from) <= 180_000_000 && bps(wire, to - from) <= 94_000_000);

    let ranges: Vec<&Request> = sim.net.finished.iter().map(|(_, r)| r).filter(|r| asked(r) > PROOF).collect();
    assert!(ranges.windows(2).any(|pair| pair[1].first < pair[0].first), "a later range finished before an earlier one");
    assert!(sim.peak_held > 0 && sim.peak_held <= sim.room, "held {} of {}", sim.peak_held, sim.room);
    assert_no_byte_twice(&sim);
}

#[test]
fn a_link_whose_proof_is_not_plainly_the_asked_range_never_carries() {
    for (answer, logged) in [
        (Answer::IgnoresRange, "wlan0 rejected: range ignored"),
        (Answer::LiesAboutTotal, "wlan0 rejected: another total size"),
        (Answer::LiesAboutOffset, "wlan0 rejected: another range than asked"),
        (Answer::NewValidator, "wlan0 rejected: another validator"),
        (Answer::Redirects, "wlan0 rejected: redirected"),
        (Answer::WeakEtagOnly, "wlan0 rejected: no validator"),
        (Answer::Chunked, "wlan0 rejected: chunked"),
        (Answer::Encoded, "wlan0 rejected: content-encoded"),
        (Answer::Overruns, "wlan0 rejected: more bytes than asked"),
        (Answer::OtherBytes, "wlan0 rejected: other bytes"),
        (Answer::TlsFails, "wlan0 not proven (TLS), again in 2 s"),
        (Answer::Truncates(1_000), "wlan0 not proven (cut short), again in 2 s"),
        (Answer::Silent, "wlan0 not proven (no answer), again in 2 s"),
    ] {
        let paths = vec![Path::new(WIRE, mbit(94)), Path::new(WIFI, mbit(180)).answering(answer, 0)];
        let mut sim = Sim::new(2 << 20, paths, vec![wire(), wifi()]);
        sim.finish(3_000);
        assert_whole(&sim);
        assert!(sim.logged(logged), "{answer:?} is logged as {logged:?}\n{}", sim.logs.join("\n"));
        assert_eq!((sim.carried, sim.handbacks), (0, 0), "{answer:?}: the default reader read it all");
        let asked_of_wifi = sim.net.requests(WIFI);
        assert!(asked_of_wifi.iter().all(|r| asked(r) == PROOF), "{answer:?}: only proofs were asked of it");
        if logged.contains("rejected") {
            assert_eq!(asked_of_wifi.len(), 1, "{answer:?}: a rejected link is not asked again");
        }
    }
}

#[test]
fn an_answer_that_is_not_a_part_of_the_file_ends_multi_link_for_it() {
    for (answer, why) in [
        (Answer::IgnoresRange, "range ignored on wlan0"),
        (Answer::LiesAboutTotal, "another total size on wlan0"),
        (Answer::LiesAboutOffset, "another range than asked on wlan0"),
        (Answer::NewValidator, "validator changed on wlan0"),
        (Answer::Redirects, "redirected on wlan0"),
        (Answer::Chunked, "chunked on wlan0"),
        (Answer::Encoded, "content-encoded on wlan0"),
        (Answer::Overruns, "more bytes than asked on wlan0"),
    ] {
        // Its proof and one range are answered well.
        let paths = vec![Path::new(WIRE, mbit(94)), Path::new(WIFI, mbit(180)).answering(answer, 2)];
        let mut sim = Sim::new(3 << 20, paths, vec![wire(), wifi()]);
        sim.finish(4_000);
        assert_whole(&sim);
        let off = sim.logged_at(&format!("off for this file: {why}")).unwrap_or_else(|| panic!("{answer:?} ends it\n{}", sim.logs.join("\n")));
        assert_eq!(sim.handbacks, 1, "{answer:?}: the default reader takes the stream back once");
        assert!(sim.carried > 0 && sim.passed > 0);
        assert!(sim.net.started.iter().all(|(at, _, _)| *at <= off * MS), "{answer:?}: nothing is asked of any link after it");
        assert!(sim.net.live().is_empty(), "{answer:?}: nothing is left in flight");
    }
}

#[test]
fn a_request_names_the_object_and_a_redirect_is_never_followed() {
    let paths = vec![Path::new(WIRE, mbit(94)), Path::new(WIFI, mbit(180)).answering(Answer::Redirects, 2)];
    let mut sim = Sim::new(3 << 20, paths, vec![wire(), wifi()]);
    sim.finish(4_000);
    // A request is a range of the one object its transport was built for and nothing else. Once
    // a proof has named the object each says so, so a server whose file changed answers 200.
    let named = sim.logged_at("carrying from byte").expect("took over") * MS;
    for (at, request, if_range) in &sim.net.started {
        assert!(request.first <= request.last && request.last < sim.total());
        assert!([WIRE, WIFI].contains(&request.source));
        if *at > named {
            assert_eq!(if_range.as_deref(), Some(ETAG));
        }
    }
    assert_eq!(sim.net.requests(WIFI).len(), 3, "the redirect is the last thing asked of that link");
}

#[test]
fn a_range_cut_short_or_refused_by_tls_is_finished_by_the_other_link() {
    for (answer, why) in [(Answer::Truncates(100_000), "cut short"), (Answer::TlsFails, "TLS")] {
        let paths = vec![Path::new(WIRE, mbit(94)), Path::new(WIFI, mbit(180)).answering(answer, 2)];
        let mut sim = Sim::new(4 << 20, paths, vec![wire(), wifi()]);
        sim.finish(4_000);
        assert_whole(&sim);
        assert!(sim.logged(&format!("wlan0 failed ({why}), failovers 1")), "{answer:?}\n{}", sim.logs.join("\n"));
        assert_eq!(sim.handbacks, 0, "{answer:?}: the wire carries on");
        assert!(!sim.logged("off for this file"));
        assert_no_byte_twice(&sim);
    }
}

#[test]
fn a_link_that_stalls_mid_range_loses_it_to_the_other_within_a_second() {
    let mut sim = wire_and_wifi(16 << 20);
    let stalled = carrying_both(&mut sim);
    sim.net.path(WIFI).rates.push((stalled, 0));
    let victim = sim.net.live().into_iter().find(|r| r.source == WIFI).expect("a range on Wi-Fi");
    sim.run_ms(1_000);

    let cancelled = sim.net.cancelled.iter().find(|(_, id)| *id == victim.id).expect("the stalled range is cancelled").0;
    assert!(cancelled - stalled <= 510 * MS, "cancelled {} ms after the last byte", (cancelled - stalled) / MS);
    let got = sim.net.spans().into_iter().find(|(r, _)| r.id == victim.id).expect("its span").1;
    let (at, again, _) = sim.net.started.iter().find(|(_, r, _)| r.id > victim.id && r.last == victim.last).expect("the rest is asked again");
    assert_eq!((again.source, again.first), (WIRE, victim.first + got), "the wire is asked for exactly what is missing");
    assert!(*at - stalled <= 1_000 * MS, "asked again {} ms after the last byte", (*at - stalled) / MS);
    assert!(sim.logged("wlan0 failed (stalled), failovers 1"), "{}", sim.logs.join("\n"));

    // What the cancelled request still says changes nothing: its range has another owner.
    let held = (sim.engine.held_bytes(), sim.engine.readable(), sim.engine.in_flight());
    sim.put(Event::Body { id: victim.id, bytes: vec![0xEE; 4_096] });
    sim.put(Event::Done { id: victim.id });
    assert_eq!((sim.engine.held_bytes(), sim.engine.readable(), sim.engine.in_flight()), held);

    sim.finish(4_000);
    assert_whole(&sim);
    assert_no_byte_twice(&sim);
    assert_eq!(sim.engine.phase(), Phase::Carrying);
}

#[test]
fn work_moves_to_the_wire_as_wifi_slows_and_the_reserve_runs_down() {
    let mut sim = wire_and_wifi(56 << 20);
    sim.film = Some(Film::new(90, 30_000));
    assert!(sim.run_until(3_000, |s| s.logged("carrying from byte")), "{}", sim.logs.join("\n"));
    // The reserve is full from here: the demuxer reads only as the film plays.
    sim.film.as_mut().expect("a film").queued = 60_000 * MS;
    let alone = |s: &Sim| s.logs.iter().any(|l| l.contains("using wlan0") && !l.contains("eth0"));
    assert!(sim.run_until(1_000, alone), "Wi-Fi covers the film alone\n{}", sim.logs.join("\n"));
    let covered = sim.now;
    sim.run_ms(300);
    assert_eq!(sim.net.bytes(WIRE, covered + 50 * MS, sim.now), 0, "the wire is left to the television");

    // Neither link covers the film now: the wire joins, held to its share.
    let slowed = sim.now;
    sim.net.path(WIFI).rates.push((slowed, mbit(60)));
    assert!(sim.run_until(800, |s| s.logged("(held to 70)")), "the wire joins\n{}", sim.logs.join("\n"));
    let shared = sim.now;
    sim.run_ms(700);
    for from in (shared..sim.now - 500 * MS).step_by(50 * MS as usize) {
        let wire = sim.net.bytes(WIRE, from, from + 500 * MS);
        assert!(wire <= mbit(70) / 2 + (1 << 20), "the wire carried {} bit/s while it shared", bps(wire, 500 * MS));
    }

    // The reserve falls under what restores it and Wi-Fi fades: the wire alone is faster than
    // both with its share held, so it carries alone and whole.
    let faded = sim.now;
    sim.film.as_mut().expect("a film").queued = 5_000 * MS;
    sim.net.path(WIFI).rates.push((faded, mbit(8)));
    assert!(sim.run_until(800, |s| s.logged("wlan0 set aside")), "Wi-Fi is set aside\n{}", sim.logs.join("\n"));
    let whole = sim.now + 100 * MS;
    sim.run_ms(400);
    assert!(bps(sim.net.bytes(WIRE, whole, sim.now), sim.now - whole) > 85_000_000, "the wire carries all it can");

    // The reserve is all but gone: nothing waits behind a slow range.
    sim.film.as_mut().expect("a film").queued = 400 * MS;
    sim.run_ms(500);
    assert!(sim.film.as_ref().is_some_and(|f| f.starved_at.is_none()), "the film never ran dry\n{}", sim.logs.join("\n"));
    assert!(sim.longest_wait_ms <= 250, "the demuxer waited {} ms for a byte", sim.longest_wait_ms);
    assert_eq!(sim.handbacks, 0);
    sim.assert_exact();
    assert_no_byte_twice(&sim);
}

#[test]
fn an_answer_that_is_slow_to_begin_is_not_a_slow_link() {
    let mut sim = wire_and_wifi(64 << 20);
    // A seek cancels what is in flight, and what is asked next has a connection to make first.
    sim.net.path(WIRE).connect = 150 * MS;
    sim.net.path(WIFI).connect = 150 * MS;
    sim.film = Some(Film::new(90, 2_000));
    carrying_both(&mut sim);
    sim.run_ms(300);
    let sought = sim.now;
    sim.seek(24 << 20);
    sim.run_ms(1_200);

    assert!(!sim.logs.iter().any(|l| l.contains("is slow") || l.contains("failed") || l.contains("set aside")), "{}", sim.logs.join("\n"));
    let (from, to) = (sought + 400 * MS, sim.now);
    let (wire, wifi) = (bps(sim.net.bytes(WIRE, from, to), to - from), bps(sim.net.bytes(WIFI, from, to), to - from));
    assert!(wifi > 150_000_000 && wire > 60_000_000, "after the seek the links carried {wifi} and {wire} bit/s\n{}", sim.logs.join("\n"));
    assert_eq!(sim.handbacks, 0);
    sim.assert_exact();
}

#[test]
fn a_link_that_pauses_for_less_than_a_stall_carries_on() {
    let mut sim = wire_and_wifi(64 << 20);
    sim.film = Some(Film::new(90, 400));
    let paused = carrying_both(&mut sim);
    // Little is queued: a range that stops is soon in the film's way.
    sim.film.as_mut().expect("a film").queued = 150 * MS;
    sim.net.path(WIFI).rates.extend([(paused, 0), (paused + 300 * MS, mbit(180))]);
    sim.run_ms(1_200);

    assert!(!sim.logs.iter().any(|l| l.contains("failed") || l.contains("set aside")), "{}", sim.logs.join("\n"));
    let (from, to) = (paused + 600 * MS, sim.now);
    let wifi = bps(sim.net.bytes(WIFI, from, to), to - from);
    assert!(wifi > 150_000_000, "Wi-Fi carries again: {wifi} bit/s\n{}", sim.logs.join("\n"));
    assert_eq!(sim.handbacks, 0);
    sim.assert_exact();
}

#[test]
fn a_reader_that_was_elsewhere_holds_its_absence_against_no_link() {
    let mut sim = wire_and_wifi(48 << 20);
    sim.film = Some(Film::new(90, 5_000));
    carrying_both(&mut sim);
    sim.run_ms(200);
    // The demux thread is held up for longer than a stall, with every connection sound.
    sim.away(700);
    let back = sim.now + 100 * MS;
    sim.run_ms(700);

    assert!(!sim.logs.iter().any(|l| l.contains("is slow") || l.contains("failed") || l.contains("set aside")), "{}", sim.logs.join("\n"));
    let carried = bps(sim.net.bytes(WIRE, back, sim.now) + sim.net.bytes(WIFI, back, sim.now), sim.now - back);
    assert!(carried > 200_000_000, "both links carry on: {carried} bit/s\n{}", sim.logs.join("\n"));
    assert_eq!((sim.handbacks, sim.engine.phase()), (0, Phase::Carrying));
    sim.assert_exact();
}

#[test]
fn a_fill_is_logged_with_what_each_link_brought() {
    let mut sim = wire_and_wifi(64 << 20);
    // The queues are full at two seconds of film, and all but empty when the links take over.
    sim.film = Some(Film { full_ms: 2_000, ..Film::new(90, 500) });
    assert!(sim.run_until(5_000, |s| s.logged("multilink: fill ")), "a fill is logged\n{}", sim.logs.join("\n"));
    let line = sim.logs.iter().find(|l| l.contains("multilink: fill ")).expect("a fill line").clone();
    let field = |after: &str| -> u64 {
        let rest = line.split(after).nth(1).unwrap_or_else(|| panic!("{after:?} in {line:?}"));
        rest.split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("a number after {after:?} in {line:?}"))
    };
    let (all, wire, wifi) = (field("fill "), field("eth0 "), field("wlan0 "));
    let (low, high, took) = (field("reserve "), field("→"), field(" in "));

    // The line says what the links delivered between the take-over and the full queues.
    let (from, to) = (sim.now - took * MS, sim.now);
    assert_eq!(sim.logged_at("carrying from byte"), Some(from / MS), "{line}");
    let delivered = |source| bps(sim.net.bytes(source, from, to), to - from) / 1_000_000;
    assert!(wire.abs_diff(delivered(WIRE)) <= 1 && wifi.abs_diff(delivered(WIFI)) <= 1 && all.abs_diff(wire + wifi) <= 1, "{line}");
    assert!(wire > 60 && wifi > 160 && wifi <= 180, "both links at what they may carry: {line}");
    // Its share over the fill, one range it was given before Wi-Fi carried, and the pacing's credit.
    assert!(sim.net.bytes(WIRE, from, to) <= mbit(70) * took / 1_000 + (2 << 20), "the wire is held to its share: {line}");
    assert!(low < 250 && (2_000..2_250).contains(&high), "{line}");
    assert_eq!(sim.logs.iter().filter(|l| l.contains("multilink: fill ")).count(), 1);
}

#[test]
fn a_link_whose_address_or_carrier_changed_is_retired_and_proven_anew() {
    const MOVED: Ipv4Addr = Ipv4Addr::new(192, 168, 5, 32);
    for moved in [true, false] {
        let mut sim = wire_and_wifi(16 << 20);
        sim.net.paths.push(Path::new(MOVED, mbit(180)));
        let changed = carrying_both(&mut sim);
        let old = sim.net.live().into_iter().find(|r| r.source == WIFI).expect("a range on Wi-Fi");
        if moved {
            // DHCP gave the interface another address: nothing answers from the old one.
            sim.offered[1].address = MOVED;
            sim.net.path(WIFI).rates.push((changed, 0));
        } else {
            // The carrier dropped and came back: the same address, on what may be another network.
            sim.offered[1].carrier_changes += 1;
        }
        sim.run_ms(1);
        assert!(sim.net.cancelled.iter().any(|(_, id)| *id == old.id), "its range is cancelled at once");
        assert!(sim.logged("wlan0 is no longer offered"));
        sim.finish(4_000);
        assert_whole(&sim);
        assert_proven_anew(&sim, if moved { MOVED } else { WIFI }, changed);
        if moved {
            assert!(sim.net.started.iter().all(|(at, r, _)| *at <= changed || r.source != WIFI), "nothing is asked from the old address");
        }
        assert!(sim.net.bytes(WIRE, changed, changed + 100 * MS) > 0, "the wire carries on");
        assert_eq!(sim.handbacks, 0);
    }
}

#[test]
fn an_unplugged_adapter_is_retired_and_the_other_links_carry_on() {
    let paths = vec![Path::new(USB, mbit(900)), Path::new(WIRE, mbit(94)), Path::new(WIFI, mbit(180))];
    let mut sim = Sim::new(24 << 20, paths, vec![usb(), wire(), wifi()]);
    assert!(sim.run_until(3_000, |s| s.logged("using eth1")), "the adapter carries\n{}", sim.logs.join("\n"));
    let unplugged = sim.now;
    sim.offered.remove(0);
    sim.net.path(USB).rates.push((unplugged, 0));
    sim.finish(5_000);
    assert_whole(&sim);
    assert!(sim.logged("eth1 is no longer offered"));
    assert!(sim.net.started.iter().all(|(at, r, _)| *at <= unplugged || r.source != USB), "nothing is asked of it again");
    assert!(sim.net.bytes(WIRE, unplugged, sim.now) > 0, "the wire carries on");
    assert_eq!(sim.handbacks, 0);
}

fn state(value: &str) -> Vec<u8> {
    format!(r#"{{"ok":true,"api":1,"version":"0.9.0","topic":"network.multi-link","value":{value}}}"#).into_bytes()
}

/// What the helper publishes with the wire, Wi-Fi with an IPv6 address too, and an adapter
/// plugged into another network.
const ACTIVE: &str = r#"{"schema":1,"boot":7,"generation":3,"enabled":true,"active":true,"reason":"ready",
    "sampledUptimeMs":50000,"validForMs":1000,"controlInterface":"eth0","candidates":[
    {"interface":"eth0","kind":"ethernet","address":"192.168.5.27","prefix":24,"carrierChanges":1,"nominalMbps":100,"signalDbm":null,"rxBytes":9,"txBytes":9},
    {"interface":"wlan0","kind":"wifi","address":"192.168.5.31","prefix":24,"carrierChanges":4,"nominalMbps":null,"signalDbm":-48,"rxBytes":9,"txBytes":9},
    {"interface":"wlan0","kind":"wifi","address":"fd00::31","prefix":64,"carrierChanges":4,"nominalMbps":null,"signalDbm":-48,"rxBytes":9,"txBytes":9},
    {"interface":"eth1","kind":"usb-ethernet","address":"10.0.0.9","prefix":24,"carrierChanges":1,"nominalMbps":1000,"signalDbm":null,"rxBytes":9,"txBytes":9}],
    "rates":[],"ipv4Only":true,"sourceProbeRequired":true}"#;

#[test]
fn only_ipv4_addresses_on_the_wire_s_subnet_are_candidates() {
    let status = status::parse(&state(ACTIVE)).expect("a usable state");
    assert_eq!((status.boot, status.sampled_ms, status.valid_for_ms), (7, 50_000, 1_000));
    let wifi = Candidate { carrier_changes: 4, ..wifi() };
    assert_eq!(status.candidates, [wire(), wifi], "the IPv6 address and the other network are not offered");
}

#[test]
fn a_state_that_is_off_unreadable_or_of_another_schema_offers_nothing() {
    let inactive = r#"{"schema":1,"boot":7,"generation":3,"enabled":true,"active":false,"reason":"one link","sampledUptimeMs":50000,
        "validForMs":0,"controlInterface":null,"candidates":[],"rates":[],"ipv4Only":true,"sourceProbeRequired":true}"#;
    for (body, why) in [
        (state(&ACTIVE.replace(r#""enabled":true"#, r#""enabled":false"#)), Unusable::Disabled),
        (state(inactive), Unusable::Inactive),
        (state(&ACTIVE.replace(r#""schema":1"#, r#""schema":2"#)), Unusable::Schema),
        (String::from_utf8(state(ACTIVE)).expect("text").replace(r#""api":1"#, r#""api":2"#).into_bytes(), Unusable::Schema),
        (state(&ACTIVE.replace(r#""sampledUptimeMs":50000,"#, "")), Unusable::Malformed),
        (state(&ACTIVE.replace(r#""kind":"wifi""#, r#""kind":"bridge""#)), Unusable::Malformed),
        (state(&ACTIVE.replace(r#""controlInterface":"eth0""#, r#""controlInterface":"wlan0""#)), Unusable::Malformed),
        (br#"{"ok":false,"error":"unknown topic"}"#.to_vec(), Unusable::Malformed),
        (b"<html>502</html>".to_vec(), Unusable::Malformed),
    ] {
        assert_eq!(status::parse(&body), Err(why), "{}", String::from_utf8_lossy(&body));
    }
}

#[test]
fn a_state_is_believed_only_while_it_vouches_for_itself() {
    let latest = status::parse(&state(ACTIVE));
    let offers = |now_ms| status::usable(&latest, now_ms).map(|s| s.candidates.len());
    assert_eq!(offers(Some(50_000)), Ok(2));
    assert_eq!(offers(Some(51_000)), Ok(2));
    assert_eq!(offers(Some(51_001)), Err(Unusable::Stale), "older than it vouches for");
    assert_eq!(offers(Some(49_999)), Err(Unusable::Stale), "dated after now");
    // The helper's clock counts the time asleep: what was sampled before standby is old after it.
    assert_eq!(offers(Some(50_000 + 8 * 3_600_000)), Err(Unusable::Stale));
    assert_eq!(offers(None), Err(Unusable::Stale), "no clock to judge it by");

    let generous = status::parse(&state(&ACTIVE.replace(r#""validForMs":1000"#, r#""validForMs":3600000"#)));
    assert!(status::usable(&generous, Some(52_000)).is_ok());
    assert_eq!(status::usable(&generous, Some(52_001)).err(), Some(Unusable::Stale), "however long it claims");
    assert_eq!(status::usable(&Err(Unusable::Missing), Some(50_000)).err(), Some(Unusable::Missing));
}

#[test]
fn without_a_usable_state_nothing_is_asked_of_any_link() {
    let mut sim = Sim::new(1 << 20, vec![Path::new(WIRE, mbit(94)), Path::new(WIFI, mbit(180))], Vec::new());
    sim.finish(2_000);
    assert_whole(&sim);
    assert!(sim.net.started.is_empty() && sim.carried == 0, "{}", sim.logs.join("\n"));
    assert_eq!(sim.engine.next_wake(sim.now), None, "nor is a wake-up");
}

#[test]
fn a_state_that_stops_vouching_hands_the_stream_back_and_its_links_are_proven_anew() {
    let mut sim = wire_and_wifi(10 << 20);
    carrying_both(&mut sim);
    // Stale, missing or switched off: all the same to the reader.
    let offered = std::mem::take(&mut sim.offered);
    let lost = sim.now;
    sim.run_ms(1);
    assert!(sim.net.live().is_empty(), "every range is cancelled at once");
    sim.run_ms(700);
    assert!(sim.logged("handing back at byte"), "{}", sim.logs.join("\n"));
    assert_eq!(sim.handbacks, 1);
    assert!(sim.net.started.iter().all(|(at, _, _)| *at <= lost), "nothing is asked on what was known before");

    sim.offered = offered;
    let back = sim.now;
    sim.finish(8_000);
    assert_whole(&sim);
    assert_proven_anew(&sim, WIRE, back);
    assert_proven_anew(&sim, WIFI, back);
    assert!(sim.net.bytes(WIFI, back, sim.now) > PROOF, "and then they carry again");
}

#[test]
fn a_restarted_helper_s_links_are_proven_anew() {
    let mut sim = wire_and_wifi(10 << 20);
    carrying_both(&mut sim);
    sim.boot += 1;
    let restarted = sim.now;
    sim.run_ms(1);
    assert!(sim.net.live().iter().all(|r| asked(r) == PROOF), "every range is cancelled at once");
    sim.finish(8_000);
    assert_whole(&sim);
    assert_proven_anew(&sim, WIRE, restarted);
    assert_proven_anew(&sim, WIFI, restarted);
}

#[test]
fn a_seek_cancels_every_range_in_flight_and_nothing_of_them_is_read() {
    let mut sim = wire_and_wifi(24 << 20);
    carrying_both(&mut sim);
    let before = sim.net.live();
    let to = (17 << 20) + 12_345;
    sim.seek(to);
    for old in &before {
        assert!(sim.net.cancelled.iter().any(|(_, id)| *id == old.id), "range {} is cancelled", old.id);
    }
    let after = sim.net.live();
    assert!(!after.is_empty() && after.iter().all(|r| r.generation > before[0].generation && r.first >= to), "{after:?}");
    // What an old range still says is of another generation.
    sim.put(Event::Body { id: before[0].id, bytes: vec![0xEE; 4_096] });
    sim.put(Event::Done { id: before[1].id });

    sim.finish(3_000);
    sim.assert_exact();
    assert_eq!(sim.read.len(), 2);
    assert_eq!((sim.read[1].0, sim.read[1].1.len() as u64), (to, sim.total() - to), "the rest was read from where it sought");
    assert_eq!(sim.handbacks, 0);
}

#[test]
fn a_stop_while_a_link_waits_to_be_tried_again_leaves_nothing_behind() {
    let paths = vec![Path::new(WIRE, mbit(94)), Path::new(WIFI, mbit(180)).answering(Answer::Truncates(100_000), 2)];
    let mut sim = Sim::new(8 << 20, paths, vec![wire(), wifi()]);
    assert!(sim.run_until(3_000, |s| s.logged("wlan0 failed (cut short)")), "{}", sim.logs.join("\n"));
    sim.run_ms(20);
    assert!(sim.engine.next_wake(sim.now).is_some(), "Wi-Fi was to be tried again");
    let stopped = sim.now;
    sim.put(Event::Stop);
    assert!(sim.net.live().is_empty(), "every range is cancelled");
    assert_eq!((sim.engine.in_flight(), sim.engine.held_bytes(), sim.engine.next_wake(sim.now)), (0, 0, None));
    sim.run_ms(3_000);
    assert!(sim.net.started.iter().all(|(at, _, _)| *at <= stopped), "nothing is asked after the stop");
}

#[test]
fn a_file_that_ends_inside_a_range_is_read_to_its_last_byte() {
    let mut sim = wire_and_wifi((3 << 20) + 12_345);
    sim.finish(4_000);
    assert_whole(&sim);
    assert!(sim.engine.at_end());
    let last = sim.total() - 1;
    assert!(sim.net.started.iter().all(|(_, r, _)| r.last <= last), "nothing past the end is asked");
    assert!(sim.net.started.iter().any(|(_, r, _)| r.last == last && asked(r) != PROOF), "a link carried the tail");
}

#[test]
fn a_range_cut_short_again_and_again_is_finished_from_where_each_left_it() {
    let paths = vec![
        Path::new(USB, mbit(400)).answering(Answer::Truncates(300_000), 2),
        Path::new(WIRE, mbit(94)),
        Path::new(WIFI, mbit(180)).answering(Answer::Truncates(100_000), 2),
    ];
    let mut sim = Sim::new(8 << 20, paths, vec![usb(), wire(), wifi()]);
    sim.finish(4_000);
    assert_whole(&sim);
    assert!(sim.logged("failovers 2"), "{}", sim.logs.join("\n"));
    assert_eq!(sim.handbacks, 0);
    assert_no_byte_twice(&sim);
}

#[test]
fn a_full_reserve_stops_the_fetching_at_the_room_it_was_given() {
    let mut sim = wire_and_wifi(24 << 20);
    sim.room = 4 << 20;
    sim.film = Some(Film::new(40, 30_000));
    carrying_both(&mut sim);
    // The queues are full: the demuxer reads nothing.
    sim.film.as_mut().expect("a film").queued = 600_000 * MS;
    sim.run_ms(600);
    let held = sim.engine.held_bytes();
    assert!(held <= sim.room && held > sim.room - (1 << 20), "it holds what it was given room for: {held}");
    let quiet = sim.now;
    sim.run_ms(400);
    assert!(sim.net.live().is_empty() && sim.net.started.iter().all(|(at, _, _)| *at <= quiet), "nothing is asked while nothing is read");
    assert_eq!(sim.engine.held_bytes(), held);

    sim.film.as_mut().expect("a film").queued = 30_000 * MS;
    sim.finish(4_000);
    assert_whole(&sim);
    assert_no_byte_twice(&sim);
}

#[test]
fn a_reserve_with_no_room_for_ranges_gets_the_stream_back_and_is_asked_nothing() {
    let mut sim = wire_and_wifi(24 << 20);
    carrying_both(&mut sim);
    // Memory is short: the reserve has nothing for the ranges.
    sim.room = 0;
    let short = sim.now;
    sim.run_ms(700);
    assert!(sim.logged("handing back at byte") && sim.logged("the reserve has no room for ranges"), "{}", sim.logs.join("\n"));
    assert_eq!((sim.handbacks, sim.engine.held_bytes()), (1, 0));
    assert!(sim.net.started.iter().all(|(at, _, _)| *at <= short), "nothing is asked while there is no room");

    sim.room = 8 << 20;
    let back = sim.now;
    sim.finish(6_000);
    assert_whole(&sim);
    assert!(sim.net.bytes(WIFI, back, sim.now) > PROOF, "the links carry again once there is room");
}

#[test]
fn an_adapter_that_covers_the_film_alone_leaves_wifi_unasked() {
    let paths = vec![Path::new(USB, mbit(900)), Path::new(WIRE, mbit(94)), Path::new(WIFI, mbit(180))];
    let mut sim = Sim::new(24 << 20, paths, vec![usb(), wire(), wifi()]);
    sim.film = Some(Film::new(80, 4_000));
    sim.finish(4_000);
    assert_whole(&sim);
    assert!(sim.net.requests(WIFI).is_empty(), "Wi-Fi is not even proven\n{}", sim.logs.join("\n"));
    // The wire shows its rate with its first ranges, and is then left to the television.
    let carried = sim.logged_at("carrying from byte").expect("took over") * MS;
    let (usb, wire) = (sim.net.bytes(USB, carried, sim.now), sim.net.bytes(WIRE, carried, sim.now));
    assert!(wire <= 2 << 20 && usb >= 20 << 20, "the wire carried {wire} bytes beside the adapter's {usb}");
    assert_eq!(sim.net.bytes(WIRE, carried + 200 * MS, sim.now), 0);
}

#[test]
fn a_wire_too_slow_for_the_film_plays_out_the_reserve_it_has() {
    let mut sim = Sim::new(20 << 20, vec![Path::new(WIRE, mbit(20)), Path::new(WIFI, mbit(180))], vec![wire(), wifi()]);
    sim.film = Some(Film::new(80, 2_000));
    carrying_both(&mut sim);
    sim.run_ms(300);
    // What is queued, and what the reader holds for the queues, as film time.
    let film = sim.film.as_ref().expect("a film");
    let (lost, reserve) = (sim.now, film.queued + sim.engine.held_bytes() * 8 * 1_000_000 / film.bps);
    sim.net.path(WIFI).rates.push((lost, 0));
    assert!(sim.run_until(6_000, |s| s.film.as_ref().is_some_and(|f| f.starved_at.is_some())), "the reserve runs out\n{}", sim.logs.join("\n"));
    let starved = sim.film.as_ref().and_then(|f| f.starved_at).expect("starved");

    // The wire brings a quarter of what the film plays: the reserve lasts a third longer than
    // its own length, and the demuxer then waits for bytes as it does on any slow link.
    let lasted = starved - lost;
    assert!(lasted >= reserve * 4 / 3 * 80 / 100 && lasted <= reserve * 4 / 3 + 150 * MS, "{} ms of reserve lasted {} ms", reserve / MS, lasted / MS);
    assert!(sim.logged("wlan0 failed (stalled), failovers 1"), "{}", sim.logs.join("\n"));
    sim.run_ms(500);
    assert!(bps(sim.net.bytes(WIRE, starved, sim.now), sim.now - starved) > 18_000_000, "the wire still carries all it can");
    assert_eq!((sim.engine.phase(), sim.handbacks), (Phase::Carrying, 0), "and the stream stays where it is");
    sim.assert_exact();
}
