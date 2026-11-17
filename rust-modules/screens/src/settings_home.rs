//! **Settings > Libraries > Home rows**: the order of the seated profile's Home rows, and which of
//! them Home leaves out.
//!
//! The page lists every row Home offers (`DirectoryView::home_rows`), hidden ones included, in the
//! profile's recorded order (`plx_data::pms::arrange_offers`). OK picks the focused row up, Up and
//! Down carry it, OK puts it down; BACK while carrying puts it back where it was. LEFT or RIGHT
//! shows or hides the focused row. Each put-down that moved the row and each switch sends the whole
//! list as one `BrowseCmd::SetHomeRows`, which the Browse store records in the profile's Home record
//! beside its pins. The page arranges by its own last edit until the recorded layout comes back
//! changed, so a put-down does not flick back for the frames the store takes to record it.

use super::*;
use plx_data::pms::{arrange_offers, HomeRowId, HomeRowOffer, HomeRowRule};
use plx_data::stores::browse::{BrowseCmd, DirectoryView};
use plx_data::stores::{StoreCmd, StoreId};
use plx_machine::machine::{Edge, InputEvent, InputKind};

/// One listed row and its focus key. The key travels with the row as it moves and is kept by
/// identity when the list is re-derived, so moving a row never re-seats the engine's focus.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    key: RowKey,
    offer: HomeRowOffer,
}

/// The row being carried, and the order BACK puts back.
struct Held {
    id: HomeRowId,
    before: Vec<Line>,
}

struct HomeRowsState {
    /// The focused row's key — an identity, not a position.
    sel: RowKey,
    lines: Vec<Line>,
    held: Option<Held>,
}

impl LogicalState for HomeRowsState {
    fn write(&self, w: &mut Canon) {
        w.u32(self.sel.0).bool(self.held.is_some()).u32(self.lines.len() as u32);
        for line in &self.lines {
            w.u32(line.key.0).bool(line.offer.hidden);
        }
    }
    fn probe(&self, out: &mut String) {
        let hidden = self.lines.iter().filter(|line| line.offer.hidden).count();
        out.push_str(&format!(
            "home_rows sel={} rows={} hidden={hidden} held={}",
            self.sel.0, self.lines.len(), self.held.is_some()
        ));
    }
}

/// The page's rows from plain inputs (the text-fit tests call this directly).
fn home_rows_form(lines: &[Line], held: Option<&HomeRowId>) -> Form<HomeRowId, (), SettingsPage> {
    use plx_platform::i18n::msg;
    let mut section = FormSection::new("");
    if lines.is_empty() {
        section = section.note(msg::home_rows_empty());
    }
    for line in lines {
        let offer = &line.offer;
        let mut row = Row::new(offer.title.as_str()).server_label();
        if held == Some(&offer.id) {
            row = row.detail(msg::home_rows_moving_detail()).value(msg::home_rows_moving());
        } else {
            if let Some(credit) = plx_ui::fmt::shared_by(&offer.source) {
                row = row.detail(credit).server_detail();
            }
            let word = if offer.hidden { msg::home_rows_hidden() } else { msg::home_rows_shown() };
            row = row.value(word).dim(offer.hidden);
        }
        section = section.item_keyed(offer.id.clone(), line.key, RowKind::Button, (), row);
    }
    Form::new().section(section)
}

pub(super) struct HomeRowsPage {
    entry: EntryId,
    form: FormTable<HomeRowId, (), SettingsPage>,
    state: HomeRowsState,
    /// What the rows are arranged by: the recorded layout, or the page's own last edit until the
    /// recorded one changes.
    layout: Vec<HomeRowRule>,
    /// What Home and the store last published, so a change re-derives the rows.
    seen_offers: Vec<HomeRowOffer>,
    seen_layout: Vec<HomeRowRule>,
}

impl HomeRowsPage {
    pub(super) fn new(entry: EntryId, views: DirectoryView<'_>) -> Self {
        let mut page = Self {
            entry,
            form: FormTable::new(super::super::registry::BAND),
            state: HomeRowsState { sel: RowKey(0), lines: Vec::new(), held: None },
            layout: views.home_layout().to_vec(),
            seen_offers: views.home_rows().to_vec(),
            seen_layout: views.home_layout().to_vec(),
        };
        page.derive();
        page.state.sel = page.state.lines.first().map_or(RowKey(0), |line| line.key);
        page.rebuild();
        page
    }

    /// Arrange the offered rows by [`Self::layout`]. A row already listed keeps its key; a new one
    /// takes the next unused key.
    fn derive(&mut self) {
        let mut next = self.state.lines.iter().map(|line| line.key.0).max().unwrap_or(0);
        let lines = arrange_offers(&self.seen_offers, &self.layout)
            .into_iter()
            .map(|offer| {
                let kept = self.state.lines.iter().find(|line| line.offer.id == offer.id).map(|line| line.key);
                let key = kept.unwrap_or_else(|| {
                    next += 1;
                    RowKey(next)
                });
                Line { key, offer }
            })
            .collect();
        self.state.lines = lines;
    }

    /// Re-derive the table from the rows, keeping the cursor on its row by identity; the pill
    /// glides after a carried row.
    fn rebuild(&mut self) {
        let keep = self.form.selected_id().cloned();
        self.form.table.compact = false;
        let form = home_rows_form(&self.state.lines, self.state.held.as_ref().map(|held| &held.id));
        self.form.set_sliding(form, keep.as_ref());
        self.form.table.list_focused = true;
    }

    fn view(&self) -> TableScreen<'_> {
        use plx_platform::i18n::msg;
        TableScreen::new(
            Header::new(RouteLayout::screen(), Some(msg::settings_title()), msg::home_rows_title(), msg::home_rows_copy()),
            &self.form.table, GroupId(0), self.entry,
        )
        .keyed(&self.form)
        .adjusting()
        .uncommitted(self.state.held.is_some())
    }

    /// Follow what Home and the store publish. A carried row holds the list still; anything that
    /// arrived meanwhile is taken up on the frame after it is put down.
    fn follow(&mut self, views: DirectoryView<'_>) -> bool {
        if self.state.held.is_some() {
            return false;
        }
        let (offers, layout) = (views.home_rows(), views.home_layout());
        let recorded = layout != self.seen_layout.as_slice();
        if !recorded && offers == self.seen_offers.as_slice() {
            return false;
        }
        if recorded {
            self.seen_layout = layout.to_vec();
            self.layout = layout.to_vec();
        }
        self.seen_offers = offers.to_vec();
        self.derive();
        self.rebuild();
        true
    }

    /// Record the rows as they stand: the whole list, in order, each with its hidden flag.
    fn commit(&mut self, fx: &mut Effects<'_, InnerHost>) {
        self.layout = self.state.lines.iter()
            .map(|line| HomeRowRule { id: line.offer.id.clone(), hidden: line.offer.hidden })
            .collect();
        let rows = self.layout.iter().map(|rule| (rule.id.clone(), rule.hidden)).collect();
        fx.push(Fx::App(AppFx::Store(StoreId::Browse, StoreCmd::Browse(BrowseCmd::SetHomeRows(rows)))));
    }

    /// OK: pick the row up, or put the carried one down (recording it only if it moved).
    fn activate(&mut self, key: u32, fx: &mut Effects<'_, InnerHost>) {
        if form_activate(&self.form, key, fx).is_none() {
            return;
        }
        match self.state.held.take() {
            Some(held) => {
                if held.before != self.state.lines {
                    self.commit(fx);
                }
            }
            None => {
                let Some(line) = self.state.lines.iter().find(|line| line.key.0 == key) else { return };
                self.state.held = Some(Held { id: line.offer.id.clone(), before: self.state.lines.clone() });
            }
        }
        self.rebuild();
    }

    /// Carry the held row one place up (`-1`) or down (`1`); `false` at either end.
    fn carry(&mut self, by: isize) -> bool {
        let Some(held) = &self.state.held else { return false };
        let Some(at) = self.state.lines.iter().position(|line| line.offer.id == held.id) else { return false };
        let Some(to) = at.checked_add_signed(by).filter(|to| *to < self.state.lines.len()) else { return false };
        self.state.lines.swap(at, to);
        self.rebuild();
        true
    }

    /// BACK while carrying: the rows go back to the order they had when the row was picked up.
    fn put_back(&mut self) {
        if let Some(held) = self.state.held.take() {
            self.state.lines = held.before;
            self.rebuild();
        }
    }

    /// LEFT or RIGHT: show or hide the focused row, and record it.
    fn flip(&mut self, key: u32, fx: &mut Effects<'_, InnerHost>) {
        let Some(line) = self.state.lines.iter_mut().find(|line| line.key.0 == key) else { return };
        line.offer.hidden = !line.offer.hidden;
        self.commit(fx);
        self.rebuild();
    }

    /// Seat the cursor on the first row. The Settings pages share one `EntryId`, so a page that
    /// opens empty holds no focus until its first rows land, and seats them itself.
    fn seat(fx: &mut Effects<'_, InnerHost>) {
        fx.push(Fx::Deliver(
            MachineId::Instance(InstanceId(0)),
            Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::FirstInGroup(GroupId(0)) })),
        ));
    }
}

impl Machine<InnerHost> for HomeRowsPage {
    type Ev = ScreenEvent<InnerHost>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, InnerHost>, fx: &mut Effects<'_, InnerHost>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                Self::seat(fx);
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                let was_empty = self.state.lines.is_empty();
                if self.follow(cx.views) {
                    if was_empty && !self.state.lines.is_empty() {
                        Self::seat(fx);
                    }
                    fx.invalidate(Provenance::Input);
                }
                self.form.table.update(t.dt(), RouteLayout::screen().sectioned_table().h);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                form_focus(&mut self.form, to.elem);
                if let Some(key) = self.form.key_at(self.form.table.sel.max(0) as usize) {
                    self.state.sel = key;
                }
                Handled::Yes
            }
            ScreenEvent::Activate(key) => {
                self.activate(*key, fx);
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            // While a row is carried, Up and Down move it rather than the focus; past either end
            // they do nothing.
            ScreenEvent::Input(InputEvent {
                kind: InputKind::Key { key: key @ (Key::Up | Key::Down), edge, .. }, ..
            }) if self.state.held.is_some() && *edge != Edge::Up => {
                if self.carry(if *key == Key::Up { -1 } else { 1 }) {
                    fx.invalidate(Provenance::Input);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent {
                kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, ..
            }) if self.state.held.is_some() => {
                self.put_back();
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent {
                kind: InputKind::Key { key: Key::Left | Key::Right, edge, at_edge: true, .. }, ..
            }) => {
                if let Some(focused) = cx.focus.current.filter(|_| self.state.held.is_none() && *edge == Edge::Down) {
                    self.flip(focused.elem, fx);
                    fx.invalidate(Provenance::Input);
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

plx_ui::focusable_via_view!(HomeRowsPage, InnerHost, view);

impl Screen<InnerHost> for HomeRowsPage {
    fn name(&self) -> &'static str { "home-rows" }
    fn state(&self) -> &dyn LogicalState { &self.state }
    fn crumb(&self, _cx: &Cx<'_, InnerHost>) -> Option<Cow<'_, str>> { Some(Cow::Borrowed(plx_platform::i18n::msg::settings_title())) }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, InnerHost>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, InnerHost>) {
        plx_ui::screen::Part::<InnerHost>::draw(&mut self.view(), f, Rect::FULL);
    }
    fn render(&self) -> RenderStrategy { RenderStrategy::Page }
    fn focus_source(&self) -> FocusSource { FocusSource::Engine }
    fn hit_source(&self) -> HitSource { HitSource::Engine }
}

#[cfg(test)]
#[path = "settings_home_tests.rs"]
mod tests;
