//! Settings > Integrations > Requests: the switch, the address, the test, forgetting and the fit of
//! every string in every shipped language.
use super::*;
use plx_machine::present::Present;
use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

type Commands = Vec<crate::registry::PreferenceCmd>;

fn run(page: &mut SeerrPage, f: impl FnOnce(&mut SeerrPage, &mut Effects<'_, InnerHost>)) -> (Commands, Vec<bool>) {
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

fn poll(page: &mut SeerrPage) -> (bool, Commands) {
    let mut moved = false;
    let (commands, _) = run(page, |p, fx| moved = p.poll(fx));
    (moved, commands)
}

fn type_in(page: &mut SeerrPage, text: &str) {
    page.state.edit.as_mut().expect("the address is being typed").buf.edit(&TextEdit::Commit(text.into()));
}

fn page_with(url: &str, enabled: bool) -> SeerrPage {
    let mut page = SeerrPage::new(EntryId(0));
    page.state.url = url.into();
    page.state.enabled = enabled;
    page.rebuild();
    page
}

fn status(page: &SeerrPage) -> String {
    page.form.table.sections.iter().flat_map(|s| s.rows.iter()).find(|r| r.is_note()).map(|r| r.label.clone()).unwrap_or_default()
}

const HOME: &str = "https://request.nateshome.net";

#[test]
fn turning_requests_on_with_no_address_offers_the_default_one() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("", false);
    let (commands, _) = run(&mut page, |p, fx| p.activate(SeId::Enabled.key().0, fx));
    match commands.as_slice() {
        [crate::registry::PreferenceCmd::SeerrLink { change: SeerrChange::Url(url), .. }, crate::registry::PreferenceCmd::SeerrLink { change: SeerrChange::Enabled(true), .. }] => {
            assert_eq!(url, HOME);
        }
        _ => panic!("the address, then the switch"),
    }
    assert!(page.state.enabled && page.state.url == HOME, "the rows show it before the write answers");
}

#[test]
fn turning_it_off_keeps_the_address() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with(HOME, true);
    let (commands, _) = run(&mut page, |p, fx| p.activate(SeId::Enabled.key().0, fx));
    assert!(matches!(commands.as_slice(), [crate::registry::PreferenceCmd::SeerrLink { change: SeerrChange::Enabled(false), .. }]));
    assert_eq!(page.state.url, HOME);
}

#[test]
fn a_typed_address_settles_and_is_sent_as_one_edit() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("", false);
    let (_, keyboard) = run(&mut page, |p, fx| p.begin_edit(fx));
    assert_eq!(keyboard, [true], "the television's keyboard goes up");
    page.state.edit.as_mut().unwrap().buf.edit(&TextEdit::Clear);
    type_in(&mut page, "  request.nateshome.net/discover ");
    let (commands, keyboard) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    assert_eq!(keyboard, [false]);
    match commands.as_slice() {
        [crate::registry::PreferenceCmd::SeerrLink { change, .. }] => assert_eq!(*change, SeerrChange::Url(HOME.into())),
        _ => panic!("one link edit"),
    }
}

#[test]
fn an_address_that_cannot_work_says_why_and_sends_nothing() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    for (typed, problem) in [
        ("ftp://x.example.net", UrlProblem::Scheme),
        ("http://seerr.example.net", UrlProblem::Plaintext),
        ("https://bad host", UrlProblem::Host),
    ] {
        let mut page = page_with("", false);
        run(&mut page, |p, fx| p.begin_edit(fx));
        page.state.edit.as_mut().unwrap().buf.edit(&TextEdit::Clear);
        type_in(&mut page, typed);
        let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
        assert!(commands.is_empty(), "{typed}");
        assert_eq!(page.state.problem, Some(problem), "{typed}");
        assert!(page.state.url.is_empty(), "{typed}");
        assert!(!status(&page).is_empty());
    }
}

#[test]
fn a_home_network_address_may_be_plain_http() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("", false);
    run(&mut page, |p, fx| p.begin_edit(fx));
    page.state.edit.as_mut().unwrap().buf.edit(&TextEdit::Clear);
    type_in(&mut page, "http://192.168.1.50:5055/");
    let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    assert!(matches!(commands.as_slice(), [crate::registry::PreferenceCmd::SeerrLink { change: SeerrChange::Url(u), .. }] if u == "http://192.168.1.50:5055"));
}

#[test]
fn backing_out_sends_nothing() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with(HOME, true);
    run(&mut page, |p, fx| p.begin_edit(fx));
    page.state.edit.as_mut().unwrap().buf.edit(&TextEdit::Clear);
    type_in(&mut page, "https://other.example.net");
    let (commands, keyboard) = run(&mut page, |p, fx| p.finish_edit(false, fx));
    assert!(commands.is_empty() && keyboard == [false]);
    assert_eq!(page.state.url, HOME);
}

#[test]
fn test_connection_asks_once_and_reads_the_answer() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let mut unset = page_with("", false);
    let (commands, _) = run(&mut unset, |p, fx| p.activate(SeId::Test.key().0, fx));
    assert!(commands.is_empty(), "no address, nothing to test");

    let mut page = page_with(HOME, true);
    assert!(status(&page).starts_with("Not tested yet"));
    let (commands, _) = run(&mut page, |p, fx| p.activate(SeId::Test.key().0, fx));
    let [crate::registry::PreferenceCmd::SeerrTest { reply }] = commands.as_slice() else { panic!("one test") };
    assert!(status(&page).starts_with("Checking"));
    let (again, _) = run(&mut page, |p, fx| p.activate(SeId::Test.key().0, fx));
    assert!(again.is_empty(), "one test at a time");
    assert!(!poll(&mut page).0, "an unanswered test leaves the page alone");
    reply.send(Ok((Status { version: "3.5.2".into() }, Me { id: 1, name: "Nitin".into() }))).unwrap();
    assert!(poll(&mut page).0);
    assert_eq!(status(&page), "Connected \u{b7} Seerr 3.5.2, signed in as Nitin.");
}

#[test]
fn each_way_a_test_can_fail_has_its_own_words() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let words = |answer: Result<(Status, Me), SeerrError>| {
        let mut page = page_with(HOME, true);
        let (commands, _) = run(&mut page, |p, fx| p.activate(SeId::Test.key().0, fx));
        let [crate::registry::PreferenceCmd::SeerrTest { reply }] = commands.as_slice() else { panic!("one test") };
        reply.send(answer).unwrap();
        assert!(poll(&mut page).0);
        status(&page)
    };
    assert!(words(Err(SeerrError::Unreachable)).starts_with("Can't reach Seerr"));
    assert!(words(Err(SeerrError::NotSeerr)).starts_with("That is not Seerr"));
    assert_ne!(words(Err(SeerrError::NoPlexAccount)), words(Err(SeerrError::SignInRefused)));
    // a refused start (the sender dropped) reads as not reachable
    let mut page = page_with(HOME, true);
    let (commands, _) = run(&mut page, |p, fx| p.activate(SeId::Test.key().0, fx));
    drop(commands);
    assert!(poll(&mut page).0);
    assert!(status(&page).starts_with("Can't reach Seerr"));
}

#[test]
fn forgetting_takes_two_presses_and_empties_the_link() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with(HOME, true);
    let (commands, _) = run(&mut page, |p, fx| p.activate(SeId::Forget.key().0, fx));
    assert!(commands.is_empty() && page.state.armed, "the first OK arms the row");
    let (commands, _) = run(&mut page, |p, fx| p.activate(SeId::Forget.key().0, fx));
    assert!(matches!(commands.as_slice(), [crate::registry::PreferenceCmd::SeerrLink { change: SeerrChange::Forget, .. }]));
    assert!(page.state.url.is_empty() && !page.state.enabled && !page.state.armed);
    assert!(page.form.index_of_key(SeId::Forget.key()).is_none(), "nothing left to forget");
}

#[test]
fn the_page_asks_for_the_stored_link_and_adopts_the_answer() {
    let _guard = plx_base::testlock::serial();
    let mut page = SeerrPage::new(EntryId(0));
    assert!(page.state.url.is_empty() && !page.state.enabled, "nothing is shown before the answer");
    let (commands, _) = run(&mut page, |p, fx| p.ask_read(fx));
    let [crate::registry::PreferenceCmd::SeerrRead { reply }] = commands.as_slice() else { panic!("one read") };
    reply.send((HOME.into(), true)).unwrap();
    assert!(poll(&mut page).0);
    assert_eq!((page.state.url.as_str(), page.state.enabled), (HOME, true));
}

#[test]
fn a_durable_write_is_followed_by_a_read_of_what_is_stored() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with(HOME, false);
    let (commands, _) = run(&mut page, |p, fx| p.activate(SeId::Enabled.key().0, fx));
    let [crate::registry::PreferenceCmd::SeerrLink { reply, .. }] = commands.as_slice() else { panic!("one edit") };
    let (moved, out) = poll(&mut page);
    assert!(!moved && out.is_empty(), "an unanswered write asks nothing more");
    reply.send(true).unwrap();
    let (_, out) = poll(&mut page);
    assert!(out.iter().any(|c| matches!(c, crate::registry::PreferenceCmd::SeerrRead { .. })), "the read follows");
}

#[test]
fn the_summary_names_the_host_only_while_on() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let link = SeerrLink { url: HOME.into(), enabled: true, ..Default::default() };
    assert_eq!(summary(&link), "request.nateshome.net");
    assert_eq!(summary(&SeerrLink { enabled: false, ..link }), "Off");
    assert_eq!(summary(&SeerrLink::default()), "Off");
}

#[test]
fn the_root_row_opens_the_page() {
    let _guard = plx_base::testlock::serial();
    let mut form = FormTable::<RootId, Action, SettingsPage>::new(crate::registry::BAND);
    form.table.compact = false;
    form.set(root_form(&RootInputs {
        signed_in: true, multi_user: false, library_count: 0, auto_sign_in: false, trailer_autoplay: true, spoiler_protection: false,
        resolution: plx_plex::plex::session::UiResolution::Auto, resolution_offered: false,
        language: plx_platform::i18n::Preference::System,
        under_posters: plx_plex::plex::session::UnderPosters::DEFAULT, plaintext: Vec::new(),
        subtitle_preset: plx_plex::plex::session::LookPreset::Marquee,
        bazarr: plx_plex::plex::session::BazarrLink::default(),
        seerr: SeerrLink::default(),
        arr: Default::default(),
        trace: plx_platform::playback_trace::Status::Off,
    }), None);
    let i = form.index_of_key(RootId::Seerr.key()).expect("the row is on a signed-in root");
    assert_eq!(form.activate(i), Some(plx_ui::form::Activation::Push(SettingsPage::Seerr)));
}

/// **Every string fits, in every shipped language**: the rows in each state the page can be in, and
/// the status line under them.
#[test]
fn every_seerr_string_fits_in_every_language() {
    let _guard = plx_base::testlock::serial();
    let layout = RouteLayout::screen();
    let ok = Probe::Connected(Status { version: "3.5.2".into() }, Me { id: 1, name: "Nitin Philip".into() });
    let probes = [
        Probe::Idle, Probe::Testing, ok, Probe::Failed(SeerrError::Unreachable), Probe::Failed(SeerrError::NotSeerr),
        Probe::Failed(SeerrError::NoPlexAccount), Probe::Failed(SeerrError::SignInRefused), Probe::Failed(SeerrError::RateLimited),
        Probe::Failed(SeerrError::Forbidden),
    ];
    let problems = [None, Some(UrlProblem::Scheme), Some(UrlProblem::Host), Some(UrlProblem::Plaintext)];
    let mut out = Vec::new();
    for language in SHIPPED {
        let _lang = language_on_this_thread_for_test(language);
        let tag = language.tag();
        for (url, enabled) in [("", false), (HOME, false), (HOME, true)] {
            for probe in &probes {
                for problem in problems {
                    for armed in [false, true] {
                        let words = status_text(!url.is_empty(), enabled, problem, probe);
                        let mut page = FormTable::<SeId, SeId, SettingsPage>::new(crate::registry::BAND);
                        page.table.compact = false;
                        page.set(seerr_form(url, enabled, None, armed, &words), None);
                        out.extend(page.table.app_fit_failures(layout.sectioned_table().w, &format!("{tag} seerr {url:?} {enabled} {probe:?} {problem:?} {armed}")));
                    }
                }
            }
        }
        let mut page = FormTable::<SeId, SeId, SettingsPage>::new(crate::registry::BAND);
        page.table.compact = false;
        page.set(seerr_form("", false, Some("https://a.example.net"), false, ""), None);
        out.extend(page.table.app_fit_failures(layout.sectioned_table().w, &format!("{tag} seerr typing")));
    }
    plx_ui::table::assert_no_fit_failures(&out);
}
