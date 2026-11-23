//! Settings > Integrations > Radarr and Sonarr: typing, settling and sending the address and key,
//! forgetting, the secrecy of the key and the fit of every string in every shipped language.
use super::*;
use plx_machine::present::Present;
use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

type Commands = Vec<crate::registry::PreferenceCmd>;

fn run(page: &mut ArrPage, f: impl FnOnce(&mut ArrPage, &mut Effects<'_, InnerHost>)) -> (Commands, Vec<bool>) {
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

fn poll(page: &mut ArrPage) -> (bool, Commands) {
    let mut moved = false;
    let (commands, _) = run(page, |p, fx| moved = p.poll(fx));
    (moved, commands)
}

fn type_in(page: &mut ArrPage, text: &str) {
    page.state.edit.as_mut().expect("a field is being typed").buf.edit_literal(&TextEdit::Commit(text.into()));
}

fn page_with(url: &str, key_set: bool) -> ArrPage {
    let mut page = ArrPage::new(ArrKind::Radarr, EntryId(0));
    page.state.url = url.into();
    page.state.key_set = key_set;
    page.rebuild();
    page
}

fn value(page: &ArrPage, id: ArId) -> Option<String> {
    let i = page.form.index_of_key(id.key())?;
    page.form.table.sections.iter().flat_map(|s| s.rows.iter()).nth(i).and_then(|r| r.value.clone())
}

fn status(page: &ArrPage) -> String {
    page.form.table.sections.iter().flat_map(|s| s.rows.iter()).find(|r| r.is_note()).map(|r| r.label.clone()).unwrap_or_default()
}

#[test]
fn a_typed_address_settles_and_is_sent_as_one_edit_for_its_own_app() {
    let _guard = plx_base::testlock::serial();
    for kind in [ArrKind::Radarr, ArrKind::Sonarr] {
        let mut page = ArrPage::new(kind, EntryId(0));
        let (_, keyboard) = run(&mut page, |p, fx| p.begin_edit(ArId::Address, fx));
        assert_eq!(keyboard, [true], "the television's keyboard goes up");
        type_in(&mut page, "  arr.nateshome.net/settings ");
        let (commands, keyboard) = run(&mut page, |p, fx| p.finish_edit(true, fx));
        assert_eq!(keyboard, [false]);
        match commands.as_slice() {
            [crate::registry::PreferenceCmd::ArrLink { kind: sent, change, .. }] => {
                assert_eq!(*sent, kind);
                assert_eq!(*change, ArrChange::Url("https://arr.nateshome.net".into()));
            }
            _ => panic!("one link edit"),
        }
        assert_eq!(page.state.url, "https://arr.nateshome.net", "the row shows it before the write answers");
    }
}

#[test]
fn an_address_that_cannot_work_says_why_and_sends_nothing() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    for (typed, problem, words) in [
        ("ftp://x.example.net", UrlProblem::Scheme, "must start with https://"),
        ("http://radarr.example.net", UrlProblem::Plaintext, "only for addresses on your home network"),
        ("https://bad host", UrlProblem::Host, "not a valid address"),
    ] {
        let mut page = page_with("", false);
        run(&mut page, |p, fx| p.begin_edit(ArId::Address, fx));
        type_in(&mut page, typed);
        let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
        assert!(commands.is_empty(), "{typed}");
        assert_eq!(page.state.problem, Some(Problem::Url(problem)), "{typed}");
        assert!(status(&page).contains(words), "{typed}: {}", status(&page));
        assert!(page.state.url.is_empty());
    }
}

#[test]
fn the_key_is_typed_as_dots_sent_once_and_never_shown_or_hashed() {
    let _guard = plx_base::testlock::serial();
    let secret = "0123456789abcdef0123456789abcdef";
    let mut page = page_with("https://radarr.nateshome.net", false);
    run(&mut page, |p, fx| p.begin_edit(ArId::Key, fx));
    type_in(&mut page, secret);
    page.rebuild();
    let shown = value(&page, ArId::Key).unwrap();
    assert!(!shown.contains("0123"), "the draft is dots: {shown}");
    let mut probe = String::new();
    page.state.probe(&mut probe);
    assert!(!probe.contains("0123"));
    let during = page.state.hash();
    let other = {
        let mut twin = page_with("https://radarr.nateshome.net", false);
        run(&mut twin, |p, fx| p.begin_edit(ArId::Key, fx));
        type_in(&mut twin, &"z".repeat(secret.len()));
        twin.state.hash()
    };
    assert_eq!(during, other, "two keys of one length hash alike: the key is not in the state");
    let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    let [crate::registry::PreferenceCmd::ArrLink { change, .. }] = commands.as_slice() else { panic!("one key edit") };
    assert_eq!(*change, ArrChange::Key(secret.into()));
    assert!(!format!("{change:?}").contains("0123"));
    assert_eq!(value(&page, ArId::Key).unwrap(), "\u{2022}".repeat(8), "after the edit the row only says a key is set");
}

#[test]
fn a_key_that_cannot_ride_in_a_header_is_refused() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("https://radarr.nateshome.net", false);
    run(&mut page, |p, fx| p.begin_edit(ArId::Key, fx));
    type_in(&mut page, "invalid\nkey");
    let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    assert!(commands.is_empty());
    assert_eq!(page.state.problem, Some(Problem::Key));
    assert!(!page.state.key_set);
}

#[test]
fn forgetting_takes_two_presses_and_empties_both_halves() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_with("https://radarr.nateshome.net", true);
    let (commands, _) = run(&mut page, |p, fx| p.activate(ArId::Forget.key().0, fx));
    assert!(commands.is_empty() && page.state.armed, "the first OK arms the row");
    let (commands, _) = run(&mut page, |p, fx| p.activate(ArId::Forget.key().0, fx));
    assert!(matches!(commands.as_slice(), [crate::registry::PreferenceCmd::ArrLink { change: ArrChange::Forget, .. }]));
    assert!(page.state.url.is_empty() && !page.state.key_set && !page.state.armed);
    assert!(page.form.index_of_key(ArId::Forget.key()).is_none(), "nothing left to forget");
}

#[test]
fn the_page_asks_for_the_stored_link_and_a_durable_write_is_followed_by_a_read() {
    let _guard = plx_base::testlock::serial();
    let mut page = ArrPage::new(ArrKind::Sonarr, EntryId(0));
    let (commands, _) = run(&mut page, |p, fx| p.ask_read(fx));
    let [crate::registry::PreferenceCmd::ArrRead { kind, reply }] = commands.as_slice() else { panic!("one read") };
    assert_eq!(*kind, ArrKind::Sonarr);
    reply.send(("https://sonarr.nateshome.net".into(), true)).unwrap();
    assert!(poll(&mut page).0);
    assert_eq!((page.state.url.as_str(), page.state.key_set), ("https://sonarr.nateshome.net", true));
    run(&mut page, |p, fx| p.begin_edit(ArId::Address, fx));
    type_in(&mut page, "sonarr.example.net");
    let (commands, _) = run(&mut page, |p, fx| p.finish_edit(true, fx));
    let [crate::registry::PreferenceCmd::ArrLink { reply, .. }] = commands.as_slice() else { panic!("one edit") };
    assert!(poll(&mut page).1.is_empty(), "an unanswered write asks nothing more");
    reply.send(true).unwrap();
    assert!(poll(&mut page).1.iter().any(|c| matches!(c, crate::registry::PreferenceCmd::ArrRead { .. })));
}

#[test]
fn the_summary_names_the_host_only_when_both_halves_are_set() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let link = ArrLink { url: "https://radarr.nateshome.net:7878".into(), api_key: "k".into() };
    assert_eq!(summary(&link), "radarr.nateshome.net:7878");
    assert_eq!(summary(&ArrLink { api_key: String::new(), ..link }), "Not set up");
}

#[test]
fn the_root_rows_open_their_pages_and_finish_clean_up_shows_only_while_one_waits() {
    let _guard = plx_base::testlock::serial();
    let build = |cleanup: Option<&str>| {
        let mut form = FormTable::<RootId, Action, SettingsPage>::new(crate::registry::BAND);
        form.table.compact = false;
        form.set(root_form(&RootInputs {
            signed_in: true, multi_user: false, library_count: 0, auto_sign_in: false, trailer_autoplay: true, spoiler_protection: false,
            resolution: plx_plex::plex::session::UiResolution::Auto, resolution_offered: false,
            language: plx_platform::i18n::Preference::System,
            under_posters: plx_plex::plex::session::UnderPosters::DEFAULT, plaintext: Vec::new(),
            subtitle_preset: plx_plex::plex::session::LookPreset::Marquee,
            bazarr: plx_plex::plex::session::BazarrLink::default(),
            seerr: plx_plex::plex::session::SeerrLink::default(),
            arr: ArrRoot { cleanup: cleanup.map(str::to_owned), ..Default::default() },
            trace: plx_platform::playback_trace::Status::Off,
        }), None);
        form
    };
    let form = build(None);
    for (id, page) in [(RootId::Radarr, SettingsPage::Radarr), (RootId::Sonarr, SettingsPage::Sonarr)] {
        let i = form.index_of_key(id.key()).expect("the row is on a signed-in root");
        assert_eq!(form.activate(i), Some(plx_ui::form::Activation::Push(page)));
    }
    assert!(form.index_of_key(RootId::FinishCleanup.key()).is_none(), "nothing waits");
    let form = build(Some("The Matrix"));
    let i = form.index_of_key(RootId::FinishCleanup.key()).expect("a clean-up waits");
    assert_eq!(form.activate(i), Some(plx_ui::form::Activation::Action(Action::FinishCleanup)));
}

/// **Every string fits, in every shipped language**: the rows in each state the page can be in, the
/// status line under them, and the root's two new rows.
#[test]
fn every_radarr_and_sonarr_string_fits_in_every_language() {
    let _guard = plx_base::testlock::serial();
    let layout = RouteLayout::screen();
    let problems = [None, Some(Problem::Key), Some(Problem::Url(UrlProblem::Scheme)), Some(Problem::Url(UrlProblem::Host)), Some(Problem::Url(UrlProblem::Plaintext))];
    let mut out = Vec::new();
    for language in SHIPPED {
        let _lang = language_on_this_thread_for_test(language);
        let tag = language.tag();
        for (url, key_set) in [("", false), ("https://radarr.nateshome.net", false), ("https://radarr.nateshome.net", true)] {
            for problem in problems {
                for armed in [false, true] {
                    let status = status_text(!url.is_empty() && key_set, problem);
                    let mut page = FormTable::<ArId, ArId, SettingsPage>::new(crate::registry::BAND);
                    page.table.compact = false;
                    page.set(arr_form(url, key_set, None, armed, &status), None);
                    out.extend(page.table.app_fit_failures(layout.sectioned_table().w, &format!("{tag} arr {url:?} {key_set} {problem:?} {armed}")));
                }
            }
        }
        let mut page = FormTable::<ArId, ArId, SettingsPage>::new(crate::registry::BAND);
        page.table.compact = false;
        page.set(arr_form("", false, Some((ArId::Key, &"\u{2022}".repeat(MAX_DOTS))), false, ""), None);
        out.extend(page.table.app_fit_failures(layout.sectioned_table().w, &format!("{tag} arr typing a key")));
        for copy in [msg::arr_radarr_copy(), msg::arr_sonarr_copy()] {
            assert!(!copy.trim().is_empty() && copy.chars().count() <= 200, "{tag}: {copy}");
        }
    }
    plx_ui::table::assert_no_fit_failures(&out);
}
