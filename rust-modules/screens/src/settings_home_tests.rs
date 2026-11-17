//! Settings > Libraries > Home rows: carrying a row, putting it back, showing and hiding, following
//! Home, and the fit of every string in every shipped language.
use super::*;
use super::super::test_support::cx_with;
use plx_data::stores::browse::DirectorySnapshot;
use plx_machine::machine::{Source, Stamped, Tick};
use plx_machine::present::Present;
use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

fn offer(hub: &str) -> HomeRowOffer {
    HomeRowOffer {
        id: HomeRowId { sid: None, hub: hub.into(), section: 0 },
        title: hub.to_uppercase(),
        source: String::new(),
        hidden: false,
    }
}

fn directory(hubs: &[&str]) -> DirectorySnapshot {
    let mut snapshot = DirectorySnapshot::default();
    snapshot.offer_home_rows(hubs.iter().map(|hub| offer(hub)).collect());
    snapshot
}

/// Step `page` once with the focus on `elem`.
fn run(page: &mut HomeRowsPage, at: &DirectorySnapshot, elem: u32, ev: ScreenEvent<InnerHost>) -> (Handled, Vec<Stamped<InnerHost>>) {
    let mut out = Vec::new();
    let mut present = Present::new();
    let mut fx = Effects::new(&mut out, MachineId::Instance(InstanceId(0)), &mut present);
    let cx = cx_with(Some(FocusKey { entry: EntryId(0), elem }), at.view());
    let handled = page.step(&ev, &cx, &mut fx);
    (handled, out)
}

fn press(key: Key, at_edge: bool) -> ScreenEvent<InnerHost> {
    ScreenEvent::Input(InputEvent {
        at: Tick::default(),
        source: Source::Sdl,
        kind: InputKind::Key { key, sym: 0, wcode: 0, edge: Edge::Down, at_edge },
    })
}

/// The layout a step recorded, as `(hub, hidden)` in order.
fn recorded(out: &[Stamped<InnerHost>]) -> Option<Vec<(String, bool)>> {
    out.iter().find_map(|s| match &s.fx {
        Fx::App(AppFx::Store(StoreId::Browse, StoreCmd::Browse(BrowseCmd::SetHomeRows(rows)))) => {
            Some(rows.iter().map(|(id, hidden)| (id.hub.clone(), *hidden)).collect())
        }
        _ => None,
    })
}

fn order(page: &HomeRowsPage) -> Vec<&str> {
    page.state.lines.iter().map(|line| line.offer.id.hub.as_str()).collect()
}

fn rows(hubs: &[(&str, bool)]) -> Option<Vec<(String, bool)>> {
    Some(hubs.iter().map(|(hub, hidden)| (hub.to_string(), *hidden)).collect())
}

#[test]
fn ok_picks_a_row_up_down_carries_it_and_ok_records_the_new_order() {
    let home = directory(&["a", "b", "c"]);
    let mut page = HomeRowsPage::new(EntryId(0), home.view());
    let a = page.state.lines[0].key.0;
    assert_eq!(recorded(&run(&mut page, &home, a, ScreenEvent::Activate(a)).1), None, "picking up records nothing");
    for _ in 0..3 {
        // the third press is past the end: the row stays last and the focus stays on it
        assert_eq!(run(&mut page, &home, a, press(Key::Down, false)).0, Handled::Yes);
    }
    assert_eq!(order(&page), ["b", "c", "a"]);
    assert_eq!(recorded(&run(&mut page, &home, a, ScreenEvent::Activate(a)).1), rows(&[("b", false), ("c", false), ("a", false)]));
    assert_eq!(page.form.selected_id().map(|id| id.hub.as_str()), Some("a"), "the cursor rides with the row");
    assert_eq!(page.state.lines[2].key.0, a, "and so does its focus key");
}

#[test]
fn back_while_carrying_puts_the_row_back_and_back_after_that_leaves() {
    let home = directory(&["a", "b", "c"]);
    let mut page = HomeRowsPage::new(EntryId(0), home.view());
    let a = page.state.lines[0].key.0;
    run(&mut page, &home, a, ScreenEvent::Activate(a));
    run(&mut page, &home, a, press(Key::Down, false));
    let (handled, out) = run(&mut page, &home, a, press(Key::Back, false));
    assert_eq!((handled, recorded(&out)), (Handled::Yes, None));
    assert_eq!(order(&page), ["a", "b", "c"]);
    assert_eq!(run(&mut page, &home, a, press(Key::Back, false)).0, Handled::No, "with nothing carried, BACK is the surface's");
    assert_eq!(recorded(&run(&mut page, &home, a, ScreenEvent::Activate(a)).1), None);
    assert_eq!(recorded(&run(&mut page, &home, a, ScreenEvent::Activate(a)).1), None, "a row put down where it was records nothing");
}

#[test]
fn left_or_right_hides_and_shows_the_focused_row() {
    let home = directory(&["a", "b", "c"]);
    let mut page = HomeRowsPage::new(EntryId(0), home.view());
    let b = page.state.lines[1].key.0;
    assert_eq!(recorded(&run(&mut page, &home, b, press(Key::Right, true)).1), rows(&[("a", false), ("b", true), ("c", false)]));
    assert_eq!(recorded(&run(&mut page, &home, b, press(Key::Left, true)).1), rows(&[("a", false), ("b", false), ("c", false)]));
}

#[test]
fn a_put_down_order_holds_until_recorded_and_a_new_row_joins_at_the_end() {
    let home = directory(&["a", "b", "c"]);
    let mut page = HomeRowsPage::new(EntryId(0), home.view());
    let a = page.state.lines[0].key.0;
    run(&mut page, &home, a, ScreenEvent::Activate(a));
    run(&mut page, &home, a, press(Key::Down, false));
    run(&mut page, &home, a, ScreenEvent::Activate(a));
    let grown = directory(&["a", "b", "c", "d"]);
    run(&mut page, &grown, a, ScreenEvent::Tick(Tick { ms: 16, dt_us: 16_000 }));
    assert_eq!(order(&page), ["b", "a", "c", "d"]);
    assert_eq!(page.state.lines[1].key.0, a, "a row already listed keeps its key");
}

#[test]
fn every_home_rows_line_fits_its_column_in_every_language() {
    let shared = HomeRowOffer { source: "a-friend-with-a-long-plex-handle".into(), ..offer("shared") };
    let hidden = HomeRowOffer { hidden: true, ..offer("hidden") };
    let lines: Vec<Line> = [offer("a"), shared, hidden].into_iter().enumerate()
        .map(|(i, offer)| Line { key: RowKey(i as u32 + 1), offer })
        .collect();
    let frame_w = RouteLayout::screen().sectioned_table().w;
    let mut out = Vec::new();
    for language in SHIPPED {
        let _guard = language_on_this_thread_for_test(language);
        for (shape, lines, held) in [("rows", &lines[..], None), ("carrying", &lines[..], Some(&lines[1].offer.id)), ("empty", &[][..], None)] {
            let mut form = FormTable::<HomeRowId, (), SettingsPage>::new(crate::registry::BAND);
            form.table.compact = false;
            form.set(home_rows_form(lines, held), None);
            out.extend(form.table.app_fit_failures(frame_w, &format!("{} home rows {shape}", language.tag())));
        }
    }
    plx_ui::table::assert_no_fit_failures(&out);
}
