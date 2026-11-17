//! One title's page: what Seerr says of it, Play when Plex has it, and Request when it may be
//! asked for. A series offers its missing seasons as a picker (every one chosen to begin with),
//! unless Seerr is set to take whole series only. A title already asked for, by the viewer or by
//! anyone (Pulsarr asks for every Watchlist add), shows "Already requested" and nothing to press: the
//! app driver reads the title again before it sends a request, so a duplicate cannot go out even
//! from a page that was open while another request landed.
use super::cards::badge_word;
use super::*;
use super::super::family::{form_focus, TABLE_GROUP};
use plx_plex::plex::seerr::{Ask, Have, SeerrError};
use plx_platform::i18n::msg;
use plx_ui::form::{Form, FormId, FormSection, FormTable, RowKey, RowKind};
use plx_ui::route_screen::{RouteGround, RouteLayout};
use plx_ui::screen::{FocusSource, HitSource, Part};
use plx_ui::table::Row as TableRow;
use plx_ui::table_screen::{Header, TablePart, TableScreen};
use plx_ui::widgets::{StatusKind, StatusOverlay};
use plx_ui::{Env, Painter, View};

use crate::registry::TitlePage;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TId {
    Play,
    Request,
    Season(u32),
}

impl FormId for TId {
    fn key(&self) -> RowKey {
        RowKey(match self {
            TId::Play => 1,
            TId::Request => 2,
            TId::Season(n) => 100 + n,
        })
    }
}

/// How the viewer's request stands on this page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Sent {
    Idle,
    Sending,
    Done,
    Failed(SeerrError),
}

enum Load {
    Loading,
    Ready(Box<TitlePage>),
    Failed(SeerrError),
}

/// The part of the overview the header shows: whole words, ending in an ellipsis when cut.
fn blurb(overview: &str) -> String {
    const MAX: usize = 240;
    let overview = overview.trim();
    if overview.chars().count() <= MAX {
        return overview.to_string();
    }
    let cut: String = overview.chars().take(MAX).collect();
    let words = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", words.trim_end_matches([',', ';', ':', '.', ' ']))
}

/// The seasons a request could name, as the picker lists them.
fn announced(page: &TitlePage) -> impl Iterator<Item = &plx_plex::plex::seerr::Season> {
    let specials = page.rules.specials;
    page.detail.seasons.iter().filter(move |s| (s.number > 0 || specials) && s.episodes > 0)
}

/// What stands beside a season that cannot be picked.
fn season_word(season: &plx_plex::plex::seerr::Season) -> Option<&'static str> {
    match season.have {
        Have::All => Some(badge_word(Badge::Available)),
        Have::Partly => Some(badge_word(Badge::PartlyAvailable)),
        Have::Pending | Have::Processing => Some(badge_word(Badge::Requested)),
        _ if season.requested => Some(badge_word(Badge::Requested)),
        _ => None,
    }
}

/// The page's rows from plain inputs (the text-fit tests call this directly).
pub(super) fn title_form(page: &TitlePage, picked: &[u32], sent: Sent) -> Form<TId, TId, ()> {
    let ask = page.detail.ask(page.rules);
    let sending = sent == Sent::Sending;
    let mut actions = FormSection::new("");
    if page.plex.is_some() {
        actions = actions.item(
            TId::Play,
            RowKind::Button,
            TId::Play,
            TableRow::new(msg::requests_play()).detail(msg::requests_play_detail()),
        );
    }
    let partial = page.rules.partial;
    actions = match &ask {
        Ask::Movie => actions
            .item(TId::Request, RowKind::Button, TId::Request, TableRow::new(msg::requests_request()))
            .disabled(sending),
        Ask::Seasons(_) => actions
            .item(TId::Request, RowKind::Button, TId::Request, TableRow::new(msg::requests_request()))
            .disabled(sending || (partial && picked.is_empty())),
        Ask::Requested => actions
            .item(TId::Request, RowKind::Button, TId::Request, TableRow::new(msg::requests_already()))
            .disabled(true),
        // In Plex, Play already says so; otherwise Seerr alone knows it is there.
        Ask::Have if page.plex.is_none() => actions
            .item(TId::Request, RowKind::Button, TId::Request, TableRow::new(msg::requests_available()))
            .disabled(true),
        Ask::Have => actions,
        Ask::Blocked => actions
            .item(TId::Request, RowKind::Button, TId::Request, TableRow::new(msg::requests_unavailable()))
            .disabled(true),
        Ask::NotYet => actions
            .item(TId::Request, RowKind::Button, TId::Request, TableRow::new(msg::requests_not_yet()))
            .disabled(true),
    };
    let mut form = Form::new().section(actions);
    if matches!(ask, Ask::Seasons(_)) && partial {
        let mut seasons = FormSection::new(msg::requests_seasons());
        for season in announced(page) {
            let label = if season.number == 0 {
                msg::requests_specials().to_string()
            } else {
                msg::requests_season(i64::from(season.number))
            };
            let id = TId::Season(season.number);
            seasons = match season_word(season) {
                None if season.have == Have::Nothing => seasons
                    .item(id, RowKind::Toggle, id, TableRow::new(label).toggle(picked.contains(&season.number)))
                    .disabled(sending),
                word => seasons
                    .item(id, RowKind::Button, id, TableRow::new(label).value(word.unwrap_or_default()))
                    .disabled(true),
            };
        }
        form = form.section(seasons);
    }
    let note = match sent {
        Sent::Idle => None,
        Sent::Sending => Some(msg::requests_sending().to_string()),
        Sent::Done => Some(msg::requests_sent().to_string()),
        Sent::Failed(error) => Some(crate::registry::seerr_error_text(error).to_string()),
    };
    match note {
        Some(note) => form.section(FormSection::new("").note(note)),
        None => form,
    }
}

pub struct TitleScreen {
    entry: EntryId,
    kind: Kind,
    tmdb: u32,
    tx: Sender<RequestsReply>,
    inbox: Receiver<RequestsReply>,
    load: Load,
    sent: Sent,
    /// The seasons the viewer has chosen to ask for.
    picked: Vec<u32>,
    form: FormTable<TId, TId, ()>,
    heading: String,
    blurb: String,
    ground: RouteGround,
}

impl TitleScreen {
    pub fn new(entry: EntryId, kind: Kind, tmdb: u32) -> Self {
        let (tx, inbox) = channel();
        Self {
            entry,
            kind,
            tmdb,
            tx,
            inbox,
            load: Load::Loading,
            sent: Sent::Idle,
            picked: Vec::new(),
            form: FormTable::new(crate::registry::BAND),
            heading: String::new(),
            blurb: String::new(),
            ground: RouteGround::authored(),
        }
    }

    fn read<H: ContentLike>(&self, fx: &mut Effects<'_, H>) {
        fx.push(Fx::App(AppFx::Requests(RequestsCmd::Detail { kind: self.kind, tmdb: self.tmdb, reply: self.tx.clone() })));
    }

    /// The rows again from what the page holds, keeping the selection where it still is.
    fn rebuild(&mut self) {
        let keep = self.form.selected_id().copied();
        match &self.load {
            Load::Ready(page) => self.form.set(title_form(page, &self.picked, self.sent), keep.as_ref()),
            _ => self.form.set(Form::new(), None),
        }
    }

    /// Take one answer; `true` when the title should be read again (a request just went through).
    fn receive(&mut self, reply: RequestsReply) -> bool {
        match reply {
            RequestsReply::Detail { result: Ok(page), .. } => {
                let free: Vec<u32> = match page.detail.ask(page.rules) {
                    Ask::Seasons(free) => free,
                    _ => Vec::new(),
                };
                if matches!(self.load, Load::Ready(_)) {
                    self.picked.retain(|n| free.contains(n));
                } else {
                    self.picked = free;
                }
                self.heading = if page.detail.title.year.is_empty() {
                    page.detail.title.name.clone()
                } else {
                    format!("{} ({})", page.detail.title.name, page.detail.title.year)
                };
                self.blurb = blurb(&page.detail.title.overview);
                self.load = Load::Ready(Box::new(page));
                false
            }
            RequestsReply::Detail { result: Err(error), .. } => {
                if !matches!(self.load, Load::Ready(_)) {
                    self.load = Load::Failed(error);
                }
                false
            }
            RequestsReply::Requested { result: Ok(()), .. } => {
                self.sent = Sent::Done;
                true
            }
            RequestsReply::Requested { result: Err(error), .. } => {
                self.sent = Sent::Failed(error);
                // A request the page thought open may have been taken meanwhile: read it again.
                error == SeerrError::AlreadyRequested
            }
            _ => false,
        }
    }

    fn activate<H: ContentLike>(&mut self, elem: u32, fx: &mut Effects<'_, H>) -> Handled {
        let Some(index) = self.form.index_of_key(RowKey(elem)) else { return Handled::Yes };
        // A disabled row (already requested, nothing chosen) answers nothing.
        if self.form.activate(index).is_none() {
            return Handled::Yes;
        }
        let Some(&id) = self.form.id_at(index) else { return Handled::Yes };
        let Load::Ready(page) = &self.load else { return Handled::Yes };
        match id {
            TId::Play => {
                if let Some((sid, rk)) = &page.plex {
                    fx.push(Fx::App(AppFx::Content(ContentReq::Push(ContentArg::Detail { sid: *sid, rk: rk.clone() }))));
                }
            }
            TId::Season(n) => {
                match self.picked.iter().position(|p| *p == n) {
                    Some(at) => {
                        self.picked.remove(at);
                    }
                    None => {
                        self.picked.push(n);
                        self.picked.sort_unstable();
                    }
                }
                self.rebuild();
                fx.invalidate(Provenance::Input);
            }
            TId::Request => {
                let seasons = match page.detail.ask(page.rules) {
                    Ask::Movie => Vec::new(),
                    Ask::Seasons(free) if page.rules.partial => free.into_iter().filter(|n| self.picked.contains(n)).collect(),
                    Ask::Seasons(free) => free,
                    _ => return Handled::Yes,
                };
                if self.sent == Sent::Sending || (self.kind == Kind::Tv && seasons.is_empty()) {
                    return Handled::Yes;
                }
                self.sent = Sent::Sending;
                fx.push(Fx::App(AppFx::Requests(RequestsCmd::Request {
                    kind: self.kind,
                    tmdb: self.tmdb,
                    seasons,
                    reply: self.tx.clone(),
                })));
                self.rebuild();
                fx.invalidate(Provenance::Input);
            }
        }
        Handled::Yes
    }

    /// The page's `Focusable`: the table under the header.
    fn view<H: ContentLike>(&self) -> impl plx_ui::screen::Focusable<H> + '_ {
        TableScreen::new(Header::new(RouteLayout::screen(), Some(msg::requests_title()), &self.heading, &self.blurb), &self.form.table, TABLE_GROUP, self.entry)
            .keyed(&self.form)
    }
}

impl<H: ContentLike> Machine<H> for TitleScreen {
    type Ev = ScreenEvent<H>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                self.read(fx);
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                let was_empty = self.form.table.n_rows() == 0;
                let mut landed = false;
                let mut again = false;
                while let Ok(reply) = self.inbox.try_recv() {
                    again |= self.receive(reply);
                    landed = true;
                }
                if landed {
                    self.rebuild();
                    fx.invalidate(Provenance::Landing(fx.from()));
                    // The rows arrived after the page was seated: put focus on the first.
                    if was_empty && self.form.table.n_rows() > 0 {
                        fx.push(Fx::Deliver(
                            fx.from(),
                            Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::FirstInGroup(TABLE_GROUP) })),
                        ));
                    }
                }
                if again {
                    self.read(fx);
                }
                self.form.table.update(t.dt(), RouteLayout::screen().sectioned_table().h);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                form_focus(&mut self.form, to.elem);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => self.activate(*elem, fx),
            ScreenEvent::PressCommit(_) => match cx.focus.current {
                Some(key) => self.activate(key.elem, fx),
                None => Handled::Yes,
            },
            ScreenEvent::Input(input) => match &input.kind {
                InputKind::Key { key: Key::Back, edge: Edge::Down, .. } => {
                    fx.push(Fx::App(AppFx::Content(ContentReq::Back)));
                    Handled::Yes
                }
                _ => Handled::No,
            },
            ScreenEvent::Enter(_) => Handled::Yes,
            _ => Handled::No,
        }
    }
}

plx_ui::focusable_via_view!(TitleScreen, H: [ContentLike], view);

impl<H: ContentLike> Screen<H> for TitleScreen {
    fn name(&self) -> &'static str {
        "request_title"
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, H>) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(msg::requests_title()))
    }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        let p: Painter = f.painter;
        self.ground.draw_home(Painter::root());
        let layout = RouteLayout::screen();
        Header::new(layout, Some(msg::requests_title()), &self.heading, &self.blurb).paint(p, f.measure);
        let frame = layout.sectioned_table();
        if self.form.table.n_rows() == 0 {
            let env = Env::inert();
            match &self.load {
                Load::Failed(error) => {
                    let reason = std::ffi::CString::new(crate::registry::seerr_error_text(*error)).unwrap_or_default();
                    StatusOverlay::new(frame, msg::requests_failed_c(), StatusKind::Failed).reason(&reason).draw(&env, p);
                }
                // placeholder-exempt: counted where drawn (StatusOverlay::draw_geometry, Working)
                _ => StatusOverlay::new(frame, msg::requests_loading_c(), StatusKind::Working).draw(&env, p),
            }
            return;
        }
        let mut table = TablePart {
            table: &self.form.table,
            frame,
            group: TABLE_GROUP,
            entry: self.entry,
            uncommitted: false,
            has_band: false,
            adjusts: false,
            keys: Some(&self.form),
        };
        Part::<H>::draw(&mut table, f, frame);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn focus_source(&self) -> FocusSource {
        FocusSource::Engine
    }
    fn hit_source(&self) -> HitSource {
        HitSource::Engine
    }
}

impl LogicalState for TitleScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.kind as u32).u32(self.tmdb);
        c.u32(match self.load {
            Load::Loading => 0,
            Load::Ready(_) => 1,
            Load::Failed(_) => 2,
        });
        c.u32(match self.sent {
            Sent::Idle => 0,
            Sent::Sending => 1,
            Sent::Done => 2,
            Sent::Failed(_) => 3,
        });
        c.seq(self.picked.len());
        for n in &self.picked {
            c.u32(*n);
        }
        c.u32(self.form.selected_key().map_or(0, |k| k.0));
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("request-title entry={} tmdb={} rows={}", self.entry.0, self.tmdb, self.form.table.n_rows()));
    }
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
