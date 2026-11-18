//! **Settings > Libraries > Top bar**: which items the seated profile's top bar shows.
//!
//! The bar is two glass pills, so the page has two sections, one per pill (`BarItem::pill`). A row
//! is listed only when its item can exist: Movies and TV Shows when the profile has a library of
//! that type, Requests while the Requests plugin is on and has an address; Watchlist and Search
//! are always there. A switch reads ON while the item is shown. OK flips it and sends one
//! `BrowseCmd::SetBarItem`, which the Browse store records in the profile's Home record. The page
//! shows its own last edit until the store publishes a different record, so a switch does not flick
//! back for the frames the store takes to record it.

use super::*;
use plx_data::browse::SecKind;
use plx_data::stores::browse::{BrowseCmd, DirectoryView};
use plx_data::stores::{StoreCmd, StoreId};
use plx_plex::plex::top_bar::{with_item, BarHidden, BarItem, BarPill};
use plx_platform::i18n::msg;

/// A row's identity: the item it switches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct TbId(BarItem);

impl FormId for TbId {
    fn key(&self) -> RowKey {
        RowKey(1 + BarItem::ALL.iter().position(|item| *item == self.0).unwrap_or(0) as u32)
    }
}

struct TopBarState {
    /// The focused row's key — an identity, not a position.
    sel: RowKey,
    offered: Vec<BarItem>,
    /// What the switches show: the store's record, or the page's own last edit until the record changes.
    hidden: BarHidden,
}

impl LogicalState for TopBarState {
    fn write(&self, w: &mut Canon) {
        w.u32(self.sel.0).u32(self.offered.len() as u32);
        for item in BarItem::ALL {
            w.bool(self.offered.contains(&item)).bool(self.hidden.hides(item));
        }
    }
    fn probe(&self, out: &mut String) {
        let hidden = BarItem::ALL.iter().filter(|item| self.hidden.hides(**item)).count();
        out.push_str(&format!("top_bar sel={} rows={} hidden={hidden}", self.sel.0, self.offered.len()));
    }
}

fn item_name(item: BarItem) -> &'static str {
    match item {
        BarItem::Movies => msg::browse_kind_movies(),
        BarItem::Shows => msg::browse_kind_tv_shows(),
        BarItem::Requests => msg::requests_tab(),
        BarItem::Watchlist => msg::watchlist_page_title(),
        BarItem::Search => msg::browse_chrome_search(),
    }
}

/// The items that can exist for this profile, in the bar's order.
fn offered_items(views: DirectoryView<'_>, requests_ready: bool) -> Vec<BarItem> {
    BarItem::ALL.into_iter().filter(|item| match item {
        BarItem::Movies => views.tab_of_kind(SecKind::Movie).is_some(),
        BarItem::Shows => views.tab_of_kind(SecKind::Show).is_some(),
        BarItem::Requests => requests_ready,
        BarItem::Watchlist | BarItem::Search => true,
    }).collect()
}

/// The page's rows from plain inputs (the text-fit tests call this directly).
fn top_bar_form(offered: &[BarItem], hidden: BarHidden) -> Form<TbId, BarItem, SettingsPage> {
    let section = |pill: BarPill, header: &str| {
        let mut section = FormSection::new(header);
        let mut any = false;
        for item in offered.iter().copied().filter(|item| item.pill() == pill) {
            any = true;
            section = section.item(
                TbId(item), RowKind::Toggle, item,
                Row::new(item_name(item)).toggle(!hidden.hides(item)),
            );
        }
        section.visible(any)
    };
    Form::new()
        .section(section(BarPill::Middle, msg::top_bar_section_middle()))
        .section(section(BarPill::Right, msg::top_bar_section_right()))
}

pub(super) struct TopBarPage {
    entry: EntryId,
    form: FormTable<TbId, BarItem, SettingsPage>,
    state: TopBarState,
    /// What the store last published, so a change replaces the page's own edit.
    seen: BarHidden,
    requests_ready: bool,
    session_watch: plx_plex::plex::session::VisibleSessionWatch,
}

impl TopBarPage {
    pub(super) fn new(entry: EntryId, views: DirectoryView<'_>) -> Self {
        let mut session_watch = plx_plex::plex::session::VisibleSessionWatch::default();
        session_watch.changed();
        let requests_ready = Self::requests_ready();
        let mut page = Self {
            entry,
            form: FormTable::new(super::super::registry::BAND),
            state: TopBarState {
                sel: RowKey(0),
                offered: offered_items(views, requests_ready),
                hidden: views.bar_hidden(),
            },
            seen: views.bar_hidden(),
            requests_ready,
            session_watch,
        };
        page.state.sel = page.state.offered.first().map_or(RowKey(0), |item| TbId(*item).key());
        page.rebuild();
        page
    }

    fn requests_ready() -> bool {
        plx_plex::plex::session::peek_settled().is_some_and(|sess| sess.seerr().is_ready())
    }

    fn rebuild(&mut self) {
        let keep = self.form.selected_id().copied();
        self.form.table.compact = false;
        self.form.set(top_bar_form(&self.state.offered, self.state.hidden), keep.as_ref());
        self.form.table.list_focused = true;
    }

    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(
            Header::new(RouteLayout::screen(), Some(msg::settings_title()), msg::top_bar_title(), msg::top_bar_copy()),
            &self.form.table, GroupId(0), self.entry,
        ).keyed(&self.form)
    }

    /// Follow what the store and the session publish: a changed record replaces the page's own
    /// edit; a library or the Requests plugin appearing or going adds or drops its row.
    fn follow(&mut self, views: DirectoryView<'_>) -> bool {
        if self.session_watch.changed() {
            self.requests_ready = Self::requests_ready();
        }
        let offered = offered_items(views, self.requests_ready);
        let published = views.bar_hidden();
        if offered == self.state.offered && published == self.seen {
            return false;
        }
        if published != self.seen {
            self.seen = published;
            self.state.hidden = published;
        }
        self.state.offered = offered;
        self.rebuild();
        true
    }

    /// OK: flip the focused switch and record it.
    fn activate(&mut self, key: u32, fx: &mut Effects<'_, InnerHost>) {
        let Some(item) = form_activate(&self.form, key, fx) else { return };
        let hide = !self.state.hidden.hides(item);
        let names: Vec<String> = BarItem::ALL.iter().filter(|other| self.state.hidden.hides(**other))
            .map(|other| other.name().to_owned()).collect();
        self.state.hidden = BarHidden::from_names(&with_item(&names, item, hide));
        fx.push(Fx::App(AppFx::Store(StoreId::Browse, StoreCmd::Browse(BrowseCmd::SetBarItem(item, hide)))));
        self.rebuild();
    }
}

impl Machine<InnerHost> for TopBarPage {
    type Ev = ScreenEvent<InnerHost>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, InnerHost>, fx: &mut Effects<'_, InnerHost>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                // The Settings pages share one `EntryId`, so a page seats its own first row.
                fx.push(Fx::Deliver(
                    MachineId::Instance(InstanceId(0)),
                    Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::FirstInGroup(GroupId(0)) })),
                ));
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                if self.follow(cx.views) {
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
            _ => Handled::No,
        }
    }
}

plx_ui::focusable_via_view!(TopBarPage, InnerHost, view);

impl Screen<InnerHost> for TopBarPage {
    fn name(&self) -> &'static str { "top-bar" }
    fn state(&self) -> &dyn LogicalState { &self.state }
    fn crumb(&self, _cx: &Cx<'_, InnerHost>) -> Option<Cow<'_, str>> { Some(Cow::Borrowed(msg::settings_title())) }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, InnerHost>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, InnerHost>) {
        plx_ui::screen::Part::<InnerHost>::draw(&mut self.view(), f, Rect::FULL);
    }
    fn render(&self) -> RenderStrategy { RenderStrategy::Page }
    fn focus_source(&self) -> FocusSource { FocusSource::Engine }
    fn hit_source(&self) -> HitSource { HitSource::Engine }
}

#[cfg(test)]
#[path = "settings_top_bar_tests.rs"]
mod tests;
