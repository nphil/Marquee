//! Settings > Libraries > Top bar: the rows offered, the switches, the record sent, following the
//! store, and the fit of every string in every shipped language.
use super::*;
use super::super::test_support::cx_with;
use plx_data::browse::view::SectionView;
use plx_data::stores::browse::DirectorySnapshot;
use plx_machine::machine::{Stamped, Tick};
use plx_machine::present::Present;
use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

fn hidden_of(names: &[&str]) -> BarHidden {
    BarHidden::from_names(&names.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>())
}

fn section(kind: SecKind) -> SectionView {
    SectionView {
        sid: Some(plx_plex::plex::ServerId::from_raw(0)), key: 1, kind,
        row: plx_data::browse::SrcRow { pinned: true, ..Default::default() },
    }
}

fn directory(kinds: &[SecKind], hidden: BarHidden) -> DirectorySnapshot {
    DirectorySnapshot::fixture(1, 0, kinds.iter().copied().map(section).collect()).with_bar_hidden(hidden)
}

fn run(page: &mut TopBarPage, at: &DirectorySnapshot, elem: u32, ev: ScreenEvent<InnerHost>) -> Vec<Stamped<InnerHost>> {
    let mut out = Vec::new();
    let mut present = Present::new();
    let mut fx = Effects::new(&mut out, MachineId::Instance(InstanceId(0)), &mut present);
    let cx = cx_with(Some(FocusKey { entry: EntryId(0), elem }), at.view());
    page.step(&ev, &cx, &mut fx);
    out
}

fn tick() -> ScreenEvent<InnerHost> {
    ScreenEvent::Tick(Tick { ms: 16, dt_us: 16_000 })
}

fn sent(out: &[Stamped<InnerHost>]) -> Vec<(BarItem, bool)> {
    out.iter().filter_map(|s| match &s.fx {
        Fx::App(AppFx::Store(StoreId::Browse, StoreCmd::Browse(BrowseCmd::SetBarItem(item, hide)))) => Some((*item, *hide)),
        _ => None,
    }).collect()
}

/// The built table as `(header, [(title, switch on)])` per section.
fn shape(offered: &[BarItem], hidden: BarHidden) -> Vec<(String, Vec<(String, bool)>)> {
    let mut form = FormTable::<TbId, BarItem, SettingsPage>::new(crate::registry::BAND);
    form.set(top_bar_form(offered, hidden), None);
    form.table.sections.iter().map(|section| {
        let rows = section.rows.iter().map(|row| (row.label.to_string(), row.toggle.expect("every row is a switch"))).collect();
        (section.header.clone(), rows)
    }).collect()
}

#[test]
fn only_items_that_can_exist_are_offered() {
    let both = directory(&[SecKind::Movie, SecKind::Show], BarHidden::none());
    let movies = directory(&[SecKind::Movie], BarHidden::none());
    let none = directory(&[], BarHidden::none());
    assert_eq!(offered_items(both.view(), true), BarItem::ALL);
    assert_eq!(offered_items(both.view(), false), [BarItem::Movies, BarItem::Shows, BarItem::Watchlist, BarItem::Search]);
    assert_eq!(offered_items(movies.view(), false), [BarItem::Movies, BarItem::Watchlist, BarItem::Search]);
    assert_eq!(offered_items(none.view(), true), [BarItem::Requests, BarItem::Watchlist, BarItem::Search]);
}

#[test]
fn the_form_lists_the_offered_items_in_their_pills_and_a_switch_is_on_while_shown() {
    let name = |item| item_name(item).to_string();
    assert_eq!(shape(&BarItem::ALL, hidden_of(&["shows", "search"])), [
        (msg::top_bar_section_middle().to_string(), vec![
            (name(BarItem::Movies), true), (name(BarItem::Shows), false), (name(BarItem::Requests), true)]),
        (msg::top_bar_section_right().to_string(), vec![(name(BarItem::Watchlist), true), (name(BarItem::Search), false)]),
    ]);
    // a pill with nothing offered leaves no header behind
    let right_only = shape(&[BarItem::Watchlist, BarItem::Search], BarHidden::none());
    assert_eq!(right_only.len(), 1);
    assert_eq!(right_only[0].0, msg::top_bar_section_right());
}

#[test]
fn ok_flips_the_row_and_sends_exactly_that_item() {
    let home = directory(&[SecKind::Movie], BarHidden::none());
    let mut page = TopBarPage::new(EntryId(0), home.view());
    let movies = TbId(BarItem::Movies).key().0;
    assert_eq!(sent(&run(&mut page, &home, movies, ScreenEvent::Activate(movies))), [(BarItem::Movies, true)]);
    assert!(page.state.hidden.hides(BarItem::Movies), "the switch shows the edit before the store publishes");
    assert_eq!(sent(&run(&mut page, &home, movies, ScreenEvent::Activate(movies))), [(BarItem::Movies, false)]);
    assert!(!page.state.hidden.hides(BarItem::Movies));
    let search = TbId(BarItem::Search).key().0;
    assert_eq!(sent(&run(&mut page, &home, search, ScreenEvent::Activate(search))), [(BarItem::Search, true)]);
}

#[test]
fn the_page_follows_the_store_and_the_libraries() {
    let home = directory(&[SecKind::Movie], BarHidden::none());
    let mut page = TopBarPage::new(EntryId(0), home.view());
    let search = TbId(BarItem::Search).key().0;
    run(&mut page, &home, search, ScreenEvent::Activate(search));
    run(&mut page, &home, search, tick());
    assert!(page.state.hidden.hides(BarItem::Search), "an unchanged record keeps the page's own edit");

    let recorded = directory(&[SecKind::Movie], hidden_of(&["watchlist"]));
    run(&mut page, &recorded, search, tick());
    assert!(page.state.hidden.hides(BarItem::Watchlist) && !page.state.hidden.hides(BarItem::Search));

    let grown = directory(&[SecKind::Movie, SecKind::Show], hidden_of(&["watchlist"]));
    run(&mut page, &grown, search, tick());
    assert_eq!(page.state.offered, [BarItem::Movies, BarItem::Shows, BarItem::Watchlist, BarItem::Search]);
    assert_eq!(page.form.selected_id().copied(), Some(TbId(BarItem::Movies)));
}

#[test]
fn every_top_bar_line_fits_its_column_in_every_language() {
    let frame_w = RouteLayout::screen().sectioned_table().w;
    let mut out = Vec::new();
    for language in SHIPPED {
        let _guard = language_on_this_thread_for_test(language);
        for (shape, hidden) in [("shown", BarHidden::none()), ("hidden", hidden_of(&["movies", "shows", "requests", "watchlist", "search"]))] {
            let mut form = FormTable::<TbId, BarItem, SettingsPage>::new(crate::registry::BAND);
            form.table.compact = false;
            form.set(top_bar_form(&BarItem::ALL, hidden), None);
            out.extend(form.table.app_fit_failures(frame_w, &format!("{} top bar {shape}", language.tag())));
        }
    }
    plx_ui::table::assert_no_fit_failures(&out);
}
