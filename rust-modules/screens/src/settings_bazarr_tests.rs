//! Settings > Integrations > Bazarr: typing, settling and sending the address and key, the test,
//! forgetting, the secrecy of the key and the fit of every string in every shipped language.
use super::*;
use plx_machine::present::Present;
use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

type Commands = Vec<crate::registry::PreferenceCmd>;

fn run(page: &mut BazarrPage, f: impl FnOnce(&mut BazarrPage, &mut Effects<'_, InnerHost>)) -> (Commands, Vec<bool>) {
    let mut out = Vec::new();
    let mut present = Present::new();
    let mut fx = Effects::new(&mut out, MachineId::Instance(InstanceId(0)), &mut present);
    f(page, &mut fx);
    let mut commands = Vec::new();
    let mut keyboard = Vec::new();
    for event in out {
        match event.fx {
            Fx::App(AppFx::Preferences(command)) => commands.push(command),
            Fx::Deliver(_, Delivery::Keyboard { up }) => keyboard.push(up),
            _ => {}
        }
    }
    (commands, keyboard)
}

fn poll(page: &mut BazarrPage) -> (bool, Commands) {
    let mut moved = false;
    let (commands, _) = run(page, |p, fx| moved = p.poll(fx));
    (moved, commands)
}

fn type_in(page: &mut BazarrPage, text: &str) {
    page.state.edit.as_mut().expect("a field is being typed").buf.edit(&TextEdit::Commit(text.into()));
}

fn page_with(url: &str, key_set: bool) -> BazarrPage {
    let mut page = BazarrPage::new(EntryId(0));
    page.state.url = url.into();
    page.state.key_set = key_set;
    page.rebuild();
    page
}

fn value(page: &BazarrPage, id: BzId) -> Option<String> {
    let i = page.form.index_of_key(id.key())?;
    page.form.table.sections.iter().flat_map(|s| s.rows.iter()).nth(i).and_then(|r| r.value.clone())
}

fn status(page: &BazarrPage) -> String {
    page.form.table.sections.iter().flat_map(|s| s.rows.iter()).find(|r| r.is_note()).map(|r| r.label.clone()).unwrap_or_default()
}

#[test]
fn a_typed_address_settles_and_is_sent_as_one_edit() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("", false);
    let (_, keyboard) = run(&mut page, |p, fx| p.begin_edit(BzId::Address, fx));
    assert_eq!(keyboard, [true], "the television's keyboard goes up");
    type_in(&mut page, "  bazarr.nateshome.net/settings ");
    let (commands, keyboard) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    assert_eq!(keyboard, [false]);
    match commands.as_slice() {
        [crate::registry::PreferenceCmd::BazarrLink { change, .. }] => {
            assert_eq!(*change, BazarrChange::Url("https://bazarr.nateshome.net".into()));
        }
        _ => panic!("one link edit"),
    }
    assert_eq!(page.state.url, "https://bazarr.nateshome.net", "the row shows it before the write answers");
}

#[test]
fn an_address_that_cannot_work_says_why_and_sends_nothing() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    for (typed, problem, words) in [
        ("ftp://x.example.net", UrlProblem::Scheme, "must start with https://"),
        ("http://bazarr.example.net", UrlProblem::Plaintext, "only for addresses on your home network"),
        ("https://bad host", UrlProblem::Host, "not a valid address"),
    ] {
        let mut page = page_with("", false);
        run(&mut page, |p, fx| p.begin_edit(BzId::Address, fx));
        type_in(&mut page, typed);
        let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
        assert!(commands.is_empty(), "{typed}");
        assert_eq!(page.state.problem, Some(Problem::Url(problem)), "{typed}");
        assert!(status(&page).contains(words), "{typed}: {}", status(&page));
        assert!(page.state.url.is_empty());
    }
}

#[test]
fn a_home_network_address_may_be_plain_http() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("", false);
    run(&mut page, |p, fx| p.begin_edit(BzId::Address, fx));
    type_in(&mut page, "http://192.168.1.50:6767/");
    let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    assert!(matches!(commands.as_slice(), [crate::registry::PreferenceCmd::BazarrLink { change: BazarrChange::Url(u), .. }] if u == "http://192.168.1.50:6767"));
}

#[test]
fn the_key_is_typed_as_dots_sent_once_and_never_shown_or_hashed() {
    let _guard = plx_base::testlock::serial();
    let secret = "0123456789abcdef0123456789abcdef";
    let mut page = page_with("https://bazarr.nateshome.net", false);
    let before = page.state.hash();
    run(&mut page, |p, fx| p.begin_edit(BzId::Key, fx));
    type_in(&mut page, secret);
    page.rebuild();
    let shown = value(&page, BzId::Key).unwrap();
    assert_eq!(shown.chars().filter(|c| *c == '\u{2022}').count(), MAX_DOTS.min(secret.len()), "{shown}");
    assert!(!shown.contains("0123"), "the draft is dots");
    let mut probe = String::new();
    page.state.probe(&mut probe);
    assert!(!probe.contains("0123") && !format!("{:?}", page.state.edit.as_ref().map(|e| e.buf.text().len())).contains("0123"));
    let during = page.state.hash();
    assert_ne!(during, before, "typing is replay state, by length");
    type_in(&mut page, "x");
    assert_ne!(page.state.hash(), during, "by length alone");
    let other = {
        let mut twin = page_with("https://bazarr.nateshome.net", false);
        run(&mut twin, |p, fx| p.begin_edit(BzId::Key, fx));
        type_in(&mut twin, &"z".repeat(secret.len() + 1));
        twin.state.hash()
    };
    assert_eq!(page.state.hash(), other, "two keys of one length hash alike: the key is not in the state");
    let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    let [crate::registry::PreferenceCmd::BazarrLink { change, .. }] = commands.as_slice() else { panic!("one key edit") };
    assert_eq!(*change, BazarrChange::Key(format!("{secret}x")));
    assert!(!format!("{change:?}").contains("0123"));
    assert!(page.state.key_set);
    assert_eq!(value(&page, BzId::Key).unwrap(), "\u{2022}".repeat(8), "after the edit the row only says a key is set");
    assert!(page.state.edit.is_none());
}

#[test]
fn a_key_that_cannot_ride_in_a_header_is_refused() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("https://bazarr.nateshome.net", false);
    run(&mut page, |p, fx| p.begin_edit(BzId::Key, fx));
    type_in(&mut page, "has a space");
    let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    assert!(commands.is_empty());
    assert_eq!(page.state.problem, Some(Problem::Key));
    assert!(!page.state.key_set);
}

#[test]
fn backing_out_sends_nothing_and_an_empty_draft_changes_nothing() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("https://bazarr.nateshome.net", true);
    run(&mut page, |p, fx| p.begin_edit(BzId::Address, fx));
    page.state.edit.as_mut().unwrap().buf.edit(&TextEdit::Clear);
    type_in(&mut page, "https://other.example.net");
    let (commands, keyboard) = run(&mut page, |p, fx| p.finish_edit(false, fx));
    assert!(commands.is_empty() && keyboard == [false]);
    assert_eq!(page.state.url, "https://bazarr.nateshome.net");
    run(&mut page, |p, fx| p.begin_edit(BzId::Address, fx));
    page.state.edit.as_mut().unwrap().buf.edit(&TextEdit::Clear);
    let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    assert!(commands.is_empty(), "nothing typed is nothing sent");
    assert_eq!(page.state.url, "https://bazarr.nateshome.net");
}

#[test]
fn a_new_key_is_typed_afresh_and_the_address_starts_from_what_is_stored() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("https://bazarr.nateshome.net", true);
    run(&mut page, |p, fx| p.begin_edit(BzId::Key, fx));
    assert_eq!(page.state.edit.as_ref().unwrap().buf.text(), "", "the stored key is not read into the draft");
    page.state.edit = None;
    run(&mut page, |p, fx| p.begin_edit(BzId::Address, fx));
    assert_eq!(page.state.edit.as_ref().unwrap().buf.text(), "https://bazarr.nateshome.net");
}

#[test]
fn test_connection_waits_for_both_halves_then_asks_and_reads_the_answer() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let mut incomplete = page_with("https://bazarr.nateshome.net", false);
    let (commands, _) = run(&mut incomplete, |p, fx| p.activate(BzId::Test.key().0, fx));
    assert!(commands.is_empty(), "no key, nothing to test");
    assert!(status(&incomplete).starts_with("Not set up"));

    let mut page = page_with("https://bazarr.nateshome.net", true);
    assert!(status(&page).starts_with("Not tested yet"));
    let (commands, _) = run(&mut page, |p, fx| p.activate(BzId::Test.key().0, fx));
    let [crate::registry::PreferenceCmd::BazarrTest { reply }] = commands.as_slice() else { panic!("one test") };
    assert!(status(&page).starts_with("Checking"));
    let (again, _) = run(&mut page, |p, fx| p.activate(BzId::Test.key().0, fx));
    assert!(again.is_empty(), "one test at a time");
    assert!(!poll(&mut page).0, "an unanswered test leaves the page alone");
    reply.send(Ok(Status { bazarr: "1.5.1".into(), sonarr: "4.0".into(), radarr: "5.3".into() })).unwrap();
    assert!(poll(&mut page).0);
    assert_eq!(status(&page), "Connected \u{b7} Bazarr 1.5.1. Sonarr 4.0 and Radarr 5.3 are linked. \u{201c}Wrong subtitles?\u{201d} in the player now offers Bazarr.");
}

#[test]
fn each_way_a_test_can_fail_has_its_own_words() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let words = |answer: Result<Status, BazarrError>| {
        let mut page = page_with("https://bazarr.nateshome.net", true);
        let (commands, _) = run(&mut page, |p, fx| p.activate(BzId::Test.key().0, fx));
        let [crate::registry::PreferenceCmd::BazarrTest { reply }] = commands.as_slice() else { panic!("one test") };
        reply.send(answer).unwrap();
        assert!(poll(&mut page).0);
        status(&page)
    };
    assert!(words(Err(BazarrError::Unreachable)).starts_with("Can't reach Bazarr"));
    assert!(words(Err(BazarrError::TimedOut)).starts_with("Can't reach Bazarr"));
    assert!(words(Err(BazarrError::BadKey)).starts_with("Bazarr refused the key"));
    assert!(words(Err(BazarrError::NotBazarr)).starts_with("That is not Bazarr"));
    let plain = words(Ok(Status { bazarr: "1.5.1".into(), sonarr: String::new(), radarr: String::new() }));
    assert!(plain.starts_with("Connected \u{b7} Bazarr 1.5.1.") && !plain.contains("Sonarr"), "{plain}");
    // a refused start (the sender dropped) reads as not reachable
    let mut page = page_with("https://bazarr.nateshome.net", true);
    let (commands, _) = run(&mut page, |p, fx| p.activate(BzId::Test.key().0, fx));
    drop(commands);
    assert!(poll(&mut page).0);
    assert!(status(&page).starts_with("Can't reach Bazarr"));
}

#[test]
fn forgetting_takes_two_presses_and_empties_both_halves() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("https://bazarr.nateshome.net", true);
    let (commands, _) = run(&mut page, |p, fx| p.activate(BzId::Forget.key().0, fx));
    assert!(commands.is_empty() && page.state.armed, "the first OK arms the row");
    assert!(value(&page, BzId::Forget).is_some());
    let (commands, _) = run(&mut page, |p, fx| p.activate(BzId::Forget.key().0, fx));
    assert!(matches!(commands.as_slice(), [crate::registry::PreferenceCmd::BazarrLink { change: BazarrChange::Forget, .. }]));
    assert!(page.state.url.is_empty() && !page.state.key_set && !page.state.armed);
    assert!(page.form.index_of_key(BzId::Forget.key()).is_none(), "nothing left to forget");
    // another row disarms it
    let mut page = page_with("https://bazarr.nateshome.net", true);
    run(&mut page, |p, fx| p.activate(BzId::Forget.key().0, fx));
    run(&mut page, |p, fx| p.activate(BzId::Test.key().0, fx));
    assert!(!page.state.armed);
}

#[test]
fn the_page_asks_for_the_stored_link_and_adopts_the_answer() {
    let _guard = plx_base::testlock::serial();
    let mut page = BazarrPage::new(EntryId(0));
    assert!(page.state.url.is_empty() && !page.state.key_set, "nothing is shown before the answer");
    let (commands, _) = run(&mut page, |p, fx| p.ask_read(fx));
    let [crate::registry::PreferenceCmd::BazarrRead { reply }] = commands.as_slice() else { panic!("one read") };
    page.state.probe = Probe::Connected(Status { bazarr: "1".into(), sonarr: String::new(), radarr: String::new() });
    reply.send(("https://bazarr.nateshome.net".into(), true)).unwrap();
    let (moved, _) = poll(&mut page);
    assert!(moved);
    assert_eq!((page.state.url.as_str(), page.state.key_set), ("https://bazarr.nateshome.net", true));
    assert_eq!(page.state.probe, Probe::Idle, "a different link has not been tested");
    assert!(status(&page).starts_with("Not tested yet"), "{}", status(&page));
}

#[test]
fn a_durable_write_is_followed_by_a_read_of_what_is_stored() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("", false);
    run(&mut page, |p, fx| p.begin_edit(BzId::Address, fx));
    type_in(&mut page, "bazarr.nateshome.net");
    let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    let [crate::registry::PreferenceCmd::BazarrLink { reply, .. }] = commands.as_slice() else { panic!("one edit") };
    let (moved, out) = poll(&mut page);
    assert!(!moved && out.is_empty(), "an unanswered write asks nothing more");
    reply.send(true).unwrap();
    let (_, out) = poll(&mut page);
    assert!(out.iter().any(|c| matches!(c, crate::registry::PreferenceCmd::BazarrRead { .. })), "the read follows");
}

#[test]
fn the_summary_names_the_host_only_when_both_halves_are_set() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let link = BazarrLink { url: "https://bazarr.nateshome.net:6767".into(), api_key: "k".into() };
    assert_eq!(summary(&link), "bazarr.nateshome.net:6767");
    assert_eq!(summary(&BazarrLink { api_key: String::new(), ..link.clone() }), "Not set up");
    assert_eq!(summary(&BazarrLink::default()), "Not set up");
}

#[test]
fn the_root_row_opens_the_page() {
    let _guard = plx_base::testlock::serial();
    let mut form = FormTable::<RootId, Action, SettingsPage>::new(crate::registry::BAND);
    form.table.compact = false;
    form.set(root_form(&RootInputs {
        signed_in: true, multi_user: false, library_count: 0, auto_sign_in: false, trailer_autoplay: true,
        resolution: plx_plex::plex::session::UiResolution::Auto, resolution_offered: false,
        language: plx_platform::i18n::Preference::System,
        under_posters: plx_plex::plex::session::UnderPosters::DEFAULT, plaintext: Vec::new(),
        subtitle_preset: plx_plex::plex::session::LookPreset::Marquee,
        bazarr: BazarrLink::default(),
    }), None);
    let i = form.index_of_key(RootId::Bazarr.key()).expect("the row is on a signed-in root");
    assert_eq!(form.activate(i), Some(plx_ui::form::Activation::Push(SettingsPage::Bazarr)));
}

/// **Every string fits, in every shipped language**: the rows in each state the page can be in, and
/// the status line under them.
#[test]
fn every_bazarr_string_fits_in_every_language() {
    let _guard = plx_base::testlock::serial();
    let layout = RouteLayout::screen();
    let ok = Probe::Connected(Status { bazarr: "1.5.1".into(), sonarr: "4.0.14".into(), radarr: "5.17.2".into() });
    let probes = [
        Probe::Idle, Probe::Testing, ok, Probe::Failed(BazarrError::Unreachable), Probe::Failed(BazarrError::BadKey),
        Probe::Failed(BazarrError::NotBazarr),
    ];
    let problems = [None, Some(Problem::Key), Some(Problem::Url(UrlProblem::Scheme)), Some(Problem::Url(UrlProblem::Host)), Some(Problem::Url(UrlProblem::Plaintext))];
    let mut out = Vec::new();
    for language in SHIPPED {
        let _lang = language_on_this_thread_for_test(language);
        let tag = language.tag();
        for (url, key_set) in [("", false), ("https://bazarr.nateshome.net", false), ("https://bazarr.nateshome.net", true)] {
            for probe in &probes {
                for problem in problems {
                    for armed in [false, true] {
                        let status = status_text((!url.is_empty(), key_set), problem, probe);
                        let mut page = FormTable::<BzId, BzId, SettingsPage>::new(crate::registry::BAND);
                        page.table.compact = false;
                        page.set(bazarr_form(url, key_set, None, armed, &status), None);
                        out.extend(page.table.app_fit_failures(layout.sectioned_table().w, &format!("{tag} bazarr {url:?} {key_set} {probe:?} {problem:?} {armed}")));
                    }
                }
            }
        }
        // typing: the longest draft the row shows
        let mut page = FormTable::<BzId, BzId, SettingsPage>::new(crate::registry::BAND);
        page.table.compact = false;
        page.set(bazarr_form("", false, Some((BzId::Key, &"\u{2022}".repeat(MAX_DOTS))), false, ""), None);
        out.extend(page.table.app_fit_failures(layout.sectioned_table().w, &format!("{tag} bazarr typing a key")));
    }
    plx_ui::table::assert_no_fit_failures(&out);
}
