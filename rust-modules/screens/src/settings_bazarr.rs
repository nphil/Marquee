//! **Settings > Integrations > Bazarr**: where the viewer's Bazarr runs and the API key it answers
//! to, which the player's Subtitles > Wrong subtitles? > Find another with Bazarr reads.
//!
//! Both are typed on the television's own keyboard (the one Search raises; `docs/search.md` §3),
//! the address in the clear and the key as dots. A typed value settles through
//! [`BazarrLink::normalize_url`] / [`BazarrLink::normalize_key`] (the address says why it was
//! refused, and plain `http://` is accepted only for the home network) and is written as one
//! `PreferenceCmd::BazarrLink`, which the app stores in the protected half of the session record.
//!
//! **The key never rests in this page.** The stored key is not read here (the page knows whether one
//! is set); the draft lives only while it is being typed, is shown as dots, enters no log line, and
//! of the page's logical state only its length is written. Test connection asks the app, which
//! reads the stored pair itself, and the answer is Bazarr's status or why it could not be had:
//! Connected, Can't reach, Refused the key, or Not Bazarr.

use super::*;
use plx_machine::machine::{InputEvent, InputKind, TextEdit};
use plx_plex::plex::bazarr::{BazarrError, Status};
use plx_plex::plex::session::{BazarrChange, BazarrLink, UrlProblem};
use plx_platform::i18n::msg;
use plx_ui::text_buffer::TextBuffer;
use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BzId {
    Address,
    Key,
    Test,
    Forget,
}

impl FormId for BzId {
    fn key(&self) -> RowKey {
        RowKey(match self {
            BzId::Address => 1,
            BzId::Key => 2,
            BzId::Test => 3,
            BzId::Forget => 4,
        })
    }
}

/// What a typed value was refused for; the status line says it until the next edit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Problem {
    Url(UrlProblem),
    Key,
}

/// What the last Test connection learned.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Probe {
    Idle,
    Testing,
    Connected(Status),
    Failed(BazarrError),
}

/// The field being typed into, and what has been typed.
struct Edit {
    field: BzId,
    buf: TextBuffer,
}

/// The longest run of dots a masked draft shows: the row has a fixed width.
const MAX_DOTS: usize = 24;

/// The page's rows from plain inputs (the text-fit tests call this directly). `typing` is the
/// field being typed and its draft as the row shows it; `armed` the Forget row waiting for its
/// second OK.
fn bazarr_form(
    url: &str,
    key_set: bool,
    typing: Option<(BzId, &str)>,
    armed: bool,
    status: &str,
) -> Form<BzId, BzId, SettingsPage> {
    let address = match typing {
        Some((BzId::Address, draft)) => format!("{draft}|"),
        _ if url.is_empty() => msg::bazarr_not_set().to_string(),
        _ => url.to_string(),
    };
    let key = match typing {
        Some((BzId::Key, draft)) => format!("{}|", draft),
        _ if key_set => "\u{2022}".repeat(8),
        _ => msg::bazarr_not_set().to_string(),
    };
    let complete = !url.is_empty() && key_set;
    let mut rows = FormSection::new("")
        .item(
            BzId::Address, RowKind::Button, BzId::Address,
            Row::new(msg::bazarr_address()).detail(msg::bazarr_address_detail()).value(address).chevron(true),
        )
        .item(
            BzId::Key, RowKind::Button, BzId::Key,
            Row::new(msg::bazarr_key()).detail(msg::bazarr_key_detail()).value(key).chevron(true),
        )
        .item(BzId::Test, RowKind::Button, BzId::Test, Row::new(msg::bazarr_test()).dim(!complete))
        .disabled(!complete);
    if !url.is_empty() || key_set {
        let mut forget = Row::new(msg::bazarr_forget()).detail(msg::bazarr_forget_detail()).destructive(true);
        if armed {
            forget = forget.value(msg::bazarr_forget_sure());
        }
        rows = rows.item(BzId::Forget, RowKind::Button, BzId::Forget, forget);
    }
    Form::new().section(rows).section(FormSection::new("").note(status))
}

/// The status line: what the page knows of the link, in one sentence or two.
fn status_text(link_set: (bool, bool), problem: Option<Problem>, probe: &Probe) -> String {
    match problem {
        Some(Problem::Url(UrlProblem::Scheme)) => return msg::bazarr_problem_scheme().to_string(),
        Some(Problem::Url(UrlProblem::Plaintext)) => return msg::bazarr_problem_plaintext().to_string(),
        Some(Problem::Url(_)) => return msg::bazarr_problem_host().to_string(),
        Some(Problem::Key) => return msg::bazarr_problem_key().to_string(),
        None => {}
    }
    match probe {
        Probe::Testing => msg::bazarr_status_testing().to_string(),
        Probe::Connected(status) if !status.sonarr.is_empty() && !status.radarr.is_empty() => {
            msg::bazarr_status_linked(&status.radarr, &status.sonarr, &status.bazarr)
        }
        Probe::Connected(status) => msg::bazarr_status_connected(&status.bazarr),
        Probe::Failed(BazarrError::BadKey) => msg::bazarr_status_bad_key().to_string(),
        Probe::Failed(BazarrError::NotBazarr | BazarrError::NotInBazarr | BazarrError::Refused) => {
            msg::bazarr_status_not_bazarr().to_string()
        }
        Probe::Failed(_) => msg::bazarr_status_unreachable().to_string(),
        Probe::Idle if link_set == (true, true) => msg::bazarr_status_untested().to_string(),
        Probe::Idle => msg::bazarr_status_unset().to_string(),
    }
}

/// The Settings root's value for the Bazarr row: the host it is set to, or that it is not set up.
pub(super) fn summary(link: &BazarrLink) -> String {
    match link.host() {
        Some(host) if link.is_complete() => host.to_string(),
        _ => msg::bazarr_summary_unset().to_string(),
    }
}

pub(super) struct BazarrPage {
    entry: EntryId,
    form: FormTable<BzId, BzId, SettingsPage>,
    state: BzState,
    /// A write sent and not yet answered: the rows show what was typed meanwhile.
    write: Option<Receiver<bool>>,
    /// A read of the stored link asked for and not yet answered (`PreferenceCmd::BazarrRead`): sent
    /// when the page opens and again once a write is durable, so the rows only ever say what is stored.
    read: Option<Receiver<(String, bool)>>,
    test: Option<Receiver<Result<Status, BazarrError>>>,
}

struct BzState {
    sel: RowKey,
    url: String,
    key_set: bool,
    edit: Option<Edit>,
    probe: Probe,
    armed: bool,
    problem: Option<Problem>,
}

impl LogicalState for BzState {
    fn write(&self, w: &mut Canon) {
        // the key is never written, and of a draft only its length
        w.u32(self.sel.0).str(&self.url).bool(self.key_set).bool(self.armed);
        w.u32(self.edit.as_ref().map_or(0, |e| 1 + e.buf.text().len() as u32));
        w.u32(match &self.probe {
            Probe::Idle => 0,
            Probe::Testing => 1,
            Probe::Connected(_) => 2,
            Probe::Failed(_) => 3,
        });
        w.u32(match self.problem {
            None => 0,
            Some(Problem::Key) => 1,
            Some(Problem::Url(_)) => 2,
        });
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!(
            "bazarr sel={} url_set={} key_set={} editing={} probe={:?}",
            self.sel.0, !self.url.is_empty(), self.key_set, self.edit.is_some(),
            std::mem::discriminant(&self.probe)
        ));
    }
}

impl BazarrPage {
    pub(super) fn new(entry: EntryId) -> Self {
        let mut page = Self {
            entry,
            form: FormTable::new(super::super::registry::BAND),
            state: BzState {
                sel: BzId::Address.key(),
                url: String::new(),
                key_set: false,
                edit: None,
                probe: Probe::Idle,
                armed: false,
                problem: None,
            },
            write: None,
            read: None,
            test: None,
        };
        page.rebuild();
        page
    }

    fn rebuild(&mut self) {
        let keep = self.form.selected_id().copied().unwrap_or(BzId::Address);
        let draft = self.state.edit.as_ref().map(|e| {
            let text = if e.field == BzId::Key {
                "\u{2022}".repeat(e.buf.text().chars().count().min(MAX_DOTS))
            } else {
                e.buf.text().to_string()
            };
            (e.field, text)
        });
        let status = status_text((!self.state.url.is_empty(), self.state.key_set), self.state.problem, &self.state.probe);
        self.form.table.compact = false;
        let form = bazarr_form(
            &self.state.url, self.state.key_set,
            draft.as_ref().map(|(f, t)| (*f, t.as_str())), self.state.armed, &status,
        );
        self.form.set(form, Some(&keep));
        self.form.table.list_focused = true;
    }

    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(
            Header::new(RouteLayout::screen(), Some(msg::settings_title()), msg::bazarr_title(), msg::bazarr_copy()),
            &self.form.table, GroupId(0), self.entry,
        ).keyed(&self.form)
    }

    /// Ask the app for the stored link; the answer lands in [`BazarrPage::poll`].
    fn ask_read(&mut self, fx: &mut Effects<'_, InnerHost>) {
        let (reply, receipt) = std::sync::mpsc::channel();
        fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::BazarrRead { reply })));
        self.read = Some(receipt);
    }

    fn keyboard(up: bool, fx: &mut Effects<'_, InnerHost>) {
        fx.push(Fx::Deliver(MachineId::Instance(InstanceId(0)), Delivery::Keyboard { up }));
    }

    /// Send one edit of the stored link and show it meanwhile.
    fn send(&mut self, change: BazarrChange, fx: &mut Effects<'_, InnerHost>) {
        match &change {
            BazarrChange::Url(url) => self.state.url = url.clone(),
            BazarrChange::Key(_) => self.state.key_set = true,
            BazarrChange::Forget => {
                self.state.url.clear();
                self.state.key_set = false;
            }
        }
        let (reply, receipt) = std::sync::mpsc::channel();
        fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::BazarrLink { change, reply })));
        self.write = Some(receipt);
        self.state.probe = Probe::Idle;
        self.test = None;
    }

    fn begin_edit(&mut self, field: BzId, fx: &mut Effects<'_, InnerHost>) {
        // a key is typed afresh: the stored one is not shown, and is not read here
        let text = if field == BzId::Address { self.state.url.clone() } else { String::new() };
        let len = text.len();
        self.state.edit = Some(Edit { field, buf: TextBuffer::new(text, len) });
        self.state.problem = None;
        self.state.armed = false;
        Self::keyboard(true, fx);
        self.rebuild();
        fx.invalidate(Provenance::Input);
    }

    /// Finish typing: settle the draft and send it, or say why it cannot be.
    fn finish_edit(&mut self, commit: bool, fx: &mut Effects<'_, InnerHost>) {
        let Some(edit) = self.state.edit.take() else { return };
        Self::keyboard(false, fx);
        if commit && !edit.buf.text().trim().is_empty() {
            match edit.field {
                BzId::Address => match BazarrLink::normalize_url(edit.buf.text()) {
                    Ok(url) => self.send(BazarrChange::Url(url), fx),
                    Err(problem) => self.state.problem = Some(Problem::Url(problem)),
                },
                _ => match BazarrLink::normalize_key(edit.buf.text()) {
                    Some(key) => self.send(BazarrChange::Key(key), fx),
                    None => self.state.problem = Some(Problem::Key),
                },
            }
        }
        self.rebuild();
        fx.invalidate(Provenance::Input);
    }

    fn activate(&mut self, key: u32, fx: &mut Effects<'_, InnerHost>) {
        let Some(row) = form_activate(&self.form, key, fx) else { return };
        if row != BzId::Forget {
            self.state.armed = false;
        }
        match row {
            BzId::Address | BzId::Key => self.begin_edit(row, fx),
            BzId::Test => {
                if self.state.probe == Probe::Testing || self.write.is_some() {
                    return;
                }
                let (reply, receipt) = std::sync::mpsc::channel();
                fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::BazarrTest { reply })));
                self.test = Some(receipt);
                self.state.probe = Probe::Testing;
                self.state.problem = None;
                self.rebuild();
                fx.invalidate(Provenance::Input);
            }
            BzId::Forget => {
                if !self.state.armed {
                    self.state.armed = true;
                } else {
                    self.state.armed = false;
                    self.send(BazarrChange::Forget, fx);
                }
                self.rebuild();
                fx.invalidate(Provenance::Input);
            }
        }
    }

    /// Settle an answered write (which is followed by a read of what is now stored), a read and a test.
    fn poll(&mut self, fx: &mut Effects<'_, InnerHost>) -> bool {
        let mut moved = false;
        if self.write.as_ref().is_some_and(|receipt| !matches!(receipt.try_recv(), Err(TryRecvError::Empty))) {
            self.write = None;
            self.ask_read(fx);
        }
        if let Some(receipt) = &self.read {
            match receipt.try_recv() {
                Err(TryRecvError::Empty) => {}
                answer => {
                    self.read = None;
                    // a refused start leaves what the page already shows
                    if let Ok((url, key_set)) = answer {
                        if (url.as_str(), key_set) != (self.state.url.as_str(), self.state.key_set) {
                            self.state.probe = Probe::Idle;
                        }
                        (self.state.url, self.state.key_set) = (url, key_set);
                    }
                    moved = true;
                }
            }
        }
        if let Some(receipt) = &self.test {
            match receipt.try_recv() {
                Err(TryRecvError::Empty) => {}
                answer => {
                    self.test = None;
                    self.state.probe = match answer {
                        Ok(Ok(status)) => Probe::Connected(status),
                        Ok(Err(error)) => Probe::Failed(error),
                        // the app could not start the request at all
                        _ => Probe::Failed(BazarrError::Unreachable),
                    };
                    moved = true;
                }
            }
        }
        if moved {
            self.rebuild();
        }
        moved
    }
}

impl Machine<InnerHost> for BazarrPage {
    type Ev = ScreenEvent<InnerHost>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, InnerHost>, fx: &mut Effects<'_, InnerHost>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                self.ask_read(fx);
                fx.push(Fx::Deliver(
                    MachineId::Instance(InstanceId(0)),
                    Delivery::Screen(ScreenEvent::Enter(Enter::Fresh {
                        focus: FocusTarget::Elem(FocusKey { entry: self.entry, elem: self.state.sel.0 }),
                    })),
                ));
                Handled::Yes
            }
            ScreenEvent::WillLeave(_) | ScreenEvent::Unmount | ScreenEvent::Cover | ScreenEvent::Suspend => {
                // a key half typed is not sent
                if self.state.edit.take().is_some() {
                    Self::keyboard(false, fx);
                    self.rebuild();
                }
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                if self.poll(fx) {
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
                if self.state.armed && self.state.sel != BzId::Forget.key() {
                    self.state.armed = false;
                    self.rebuild();
                }
                Handled::Yes
            }
            ScreenEvent::Activate(key) => {
                if self.state.edit.is_none() {
                    self.activate(*key, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::SystemKeyboard(up), .. }) => {
                // the television's keyboard went down (its own Done): that commits what was typed
                if !*up && self.state.edit.is_some() {
                    self.finish_edit(true, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Text(edit), .. }) if self.state.edit.is_some() => {
                if let Some(typing) = self.state.edit.as_mut() {
                    typing.buf.edit(edit);
                }
                self.rebuild();
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key, sym, edge, .. }, .. })
                if self.state.edit.is_some() && *edge != plx_machine::machine::Edge::Up =>
            {
                let caret = match (*key, *sym) {
                    (Key::Left, _) => Some(TextEdit::Left),
                    (Key::Right, _) => Some(TextEdit::Right),
                    (_, plx_ui::consts::SDLK_BACKSPACE) => Some(TextEdit::Backspace),
                    (_, plx_ui::consts::SDLK_CLEAR) => Some(TextEdit::Clear),
                    _ => None,
                };
                if let Some(edit) = caret {
                    if let Some(typing) = self.state.edit.as_mut() {
                        typing.buf.edit(&edit);
                    }
                    self.rebuild();
                    fx.invalidate(Provenance::Input);
                } else if *key == Key::Back {
                    self.finish_edit(false, fx);
                } else if *key == Key::Ok {
                    self.finish_edit(true, fx);
                }
                // while a field is being typed every key is the keyboard's
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent {
                kind: InputKind::Key { key: Key::Right, at_edge: true, .. }, ..
            }) => {
                if let Some(key) = cx.focus.current.and_then(|k| form_right_target(&self.form, k.elem)) {
                    self.activate(key, fx);
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

plx_ui::focusable_via_view!(BazarrPage, InnerHost, view);

impl Screen<InnerHost> for BazarrPage {
    fn name(&self) -> &'static str { "bazarr" }
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
#[path = "settings_bazarr_tests.rs"]
mod tests;
