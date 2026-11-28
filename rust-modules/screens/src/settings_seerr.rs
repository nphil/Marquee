//! **Settings > Integrations > Requests**: whether the Requests area is on, and where the viewer's
//! Seerr runs.
//!
//! Requests appears only when it is enabled and an address has been entered. The address is
//! typed on the television's own keyboard and settles through [`SeerrLink::normalize_url`]
//! (the address says why it was refused, and plain `http://` is accepted only for the home
//! network). Every edit is one `PreferenceCmd::SeerrLink`, which the app writes into the
//! protected half of the session record.
//!
//! **The Seerr session never rests in this page.** It signs in with the seated Plex account and the
//! cookie it gets stays with the app; Test connection asks the app, which answers Seerr's version
//! and the name it signed in as, or why it could not.

use super::*;
use plx_machine::machine::{InputEvent, InputKind, TextEdit};
use plx_plex::plex::seerr::{Me, SeerrError, Status};
use plx_plex::plex::session::{SeerrChange, SeerrLink, UrlProblem};
use plx_platform::i18n::msg;
use plx_ui::text_buffer::TextBuffer;
use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SeId {
    Enabled,
    Address,
    Test,
    Forget,
}

impl FormId for SeId {
    fn key(&self) -> RowKey {
        RowKey(match self {
            SeId::Enabled => 1,
            SeId::Address => 2,
            SeId::Test => 3,
            SeId::Forget => 4,
        })
    }
}

/// What the last Test connection learned.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Probe {
    Idle,
    Testing,
    Connected(Status, Me),
    Failed(SeerrError),
}

/// The address being typed.
struct Edit {
    buf: TextBuffer,
    keyboard_up: bool,
}

/// The page's rows from plain inputs (the text-fit tests call this directly). `typing` is the
/// address draft as the row shows it; `armed` the Forget row waiting for its second OK.
fn seerr_form(
    url: &str,
    enabled: bool,
    typing: Option<&str>,
    armed: bool,
    status: &str,
) -> Form<SeId, SeId, SettingsPage> {
    let set = typing.is_some() || !url.is_empty();
    let address = match typing {
        Some(draft) => format!("{draft}|"),
        None if url.is_empty() => msg::seerr_not_set().to_string(),
        None => url.to_string(),
    };
    let mut address_row = Row::new(msg::seerr_address()).detail(msg::seerr_address_detail()).value(address).chevron(true);
    if set {
        address_row = address_row.server_value();
    }
    let mut rows = FormSection::new("")
        .item(
            SeId::Enabled, RowKind::Toggle, SeId::Enabled,
            Row::new(msg::seerr_enabled()).detail(msg::seerr_enabled_detail()).toggle(enabled),
        )
        .item(SeId::Address, RowKind::Button, SeId::Address, address_row)
        .item(SeId::Test, RowKind::Button, SeId::Test, Row::new(msg::seerr_test()).dim(url.is_empty()));
    if !url.is_empty() || enabled {
        let mut forget = Row::new(msg::seerr_forget()).detail(msg::seerr_forget_detail()).destructive(true);
        if armed {
            forget = forget.value(msg::seerr_forget_sure());
        }
        rows = rows.item(SeId::Forget, RowKind::Button, SeId::Forget, forget);
    }
    Form::new().section(rows).section(FormSection::new("").note(status))
}

/// The status line: what the page knows of the link, in one sentence or two.
fn status_text(url_set: bool, enabled: bool, problem: Option<UrlProblem>, probe: &Probe) -> String {
    match problem {
        Some(UrlProblem::Scheme) => return msg::seerr_problem_scheme().to_string(),
        Some(UrlProblem::Plaintext) => return msg::seerr_problem_plaintext().to_string(),
        Some(_) => return msg::seerr_problem_host().to_string(),
        None => {}
    }
    match probe {
        Probe::Testing => msg::seerr_status_testing().to_string(),
        Probe::Connected(status, me) => msg::seerr_status_connected(&me.name, &status.version),
        Probe::Failed(error) => super::super::registry::seerr_error_text(*error).to_string(),
        Probe::Idle if !enabled => msg::seerr_status_off().to_string(),
        Probe::Idle if url_set => msg::seerr_status_untested().to_string(),
        Probe::Idle => msg::seerr_status_unset().to_string(),
    }
}

/// The Settings root's value for the Requests row: its host while on, or that it is off.
pub(super) fn summary(link: &SeerrLink) -> String {
    match link.host() {
        Some(host) if link.enabled => host.to_string(),
        _ => msg::seerr_summary_off().to_string(),
    }
}

pub(super) struct SeerrPage {
    entry: EntryId,
    form: FormTable<SeId, SeId, SettingsPage>,
    state: SeState,
    /// Writes sent and not yet answered: the rows show what was typed meanwhile.
    writes: Vec<Receiver<bool>>,
    /// A read of the stored link asked for and not yet answered (`PreferenceCmd::SeerrRead`): sent
    /// when the page opens and again once the writes are durable, so the rows only say what is stored.
    read: Option<Receiver<(String, bool)>>,
    test: Option<Receiver<Result<(Status, Me), SeerrError>>>,
}

struct SeState {
    sel: RowKey,
    url: String,
    enabled: bool,
    edit: Option<Edit>,
    probe: Probe,
    armed: bool,
    problem: Option<UrlProblem>,
}

impl LogicalState for SeState {
    fn write(&self, w: &mut Canon) {
        w.u32(self.sel.0).str(&self.url).bool(self.enabled).bool(self.armed);
        w.u32(self.edit.as_ref().map_or(0, |e| 1 + e.buf.text().len() as u32));
        w.u32(match &self.probe {
            Probe::Idle => 0,
            Probe::Testing => 1,
            Probe::Connected(..) => 2,
            Probe::Failed(_) => 3,
        });
        w.u32(u32::from(self.problem.is_some()));
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!(
            "seerr sel={} url_set={} enabled={} editing={} probe={:?}",
            self.sel.0, !self.url.is_empty(), self.enabled, self.edit.is_some(),
            std::mem::discriminant(&self.probe)
        ));
    }
}

impl SeerrPage {
    pub(super) fn new(entry: EntryId) -> Self {
        let mut page = Self {
            entry,
            form: FormTable::new(super::super::registry::BAND),
            state: SeState {
                sel: SeId::Enabled.key(),
                url: String::new(),
                enabled: false,
                edit: None,
                probe: Probe::Idle,
                armed: false,
                problem: None,
            },
            writes: Vec::new(),
            read: None,
            test: None,
        };
        page.rebuild();
        page
    }

    fn rebuild(&mut self) {
        let keep = self.form.selected_id().copied().unwrap_or(SeId::Enabled);
        let draft = self.state.edit.as_ref().map(|e| e.buf.text().to_string());
        let status = status_text(!self.state.url.is_empty(), self.state.enabled, self.state.problem, &self.state.probe);
        self.form.table.compact = false;
        let form = seerr_form(&self.state.url, self.state.enabled, draft.as_deref(), self.state.armed, &status);
        self.form.set(form, Some(&keep));
        self.form.table.list_focused = true;
    }

    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(
            Header::new(RouteLayout::screen(), Some(msg::settings_title()), msg::seerr_title(), msg::seerr_copy()),
            &self.form.table, GroupId(0), self.entry,
        ).keyed(&self.form)
    }

    /// Ask the app for the stored link; the answer lands in [`SeerrPage::poll`].
    fn ask_read(&mut self, fx: &mut Effects<'_, InnerHost>) {
        let (reply, receipt) = std::sync::mpsc::channel();
        fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::SeerrRead { reply })));
        self.read = Some(receipt);
    }

    fn keyboard(up: bool, fx: &mut Effects<'_, InnerHost>) {
        fx.push(Fx::Deliver(MachineId::Instance(InstanceId(0)), Delivery::Keyboard { up }));
    }

    /// Send one edit of the stored link and show it meanwhile.
    fn send(&mut self, change: SeerrChange, fx: &mut Effects<'_, InnerHost>) {
        match &change {
            SeerrChange::Url(url) => self.state.url = url.clone(),
            SeerrChange::Enabled(on) => self.state.enabled = *on,
            SeerrChange::Forget => {
                self.state.url.clear();
                self.state.enabled = false;
            }
            SeerrChange::SignedIn { .. } | SeerrChange::SignedOut => {}
        }
        let (reply, receipt) = std::sync::mpsc::channel();
        fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::SeerrLink { change, reply })));
        self.writes.push(receipt);
        self.state.probe = Probe::Idle;
        self.test = None;
    }

    fn begin_edit(&mut self, fx: &mut Effects<'_, InnerHost>) {
        let text = self.state.url.clone();
        let len = text.len();
        self.state.edit = Some(Edit { buf: TextBuffer::new(text, len), keyboard_up: true });
        self.state.problem = None;
        self.state.armed = false;
        Self::keyboard(true, fx);
        self.rebuild();
        fx.invalidate(Provenance::Input);
    }

    /// Finish typing: settle the draft and send it, or say why it cannot be.
    fn finish_edit(&mut self, commit: bool, fx: &mut Effects<'_, InnerHost>) {
        let Some(mut edit) = self.state.edit.take() else { return };
        if edit.keyboard_up { Self::keyboard(false, fx); }
        edit.keyboard_up = false;
        if commit && !edit.buf.text().trim().is_empty() {
            match SeerrLink::normalize_url(edit.buf.text()) {
                Ok(url) => {
                    self.state.problem = None;
                    self.send(SeerrChange::Url(url), fx);
                }
                Err(problem) => {
                    self.state.problem = Some(problem);
                    self.state.edit = Some(edit);
                }
            }
        }
        self.rebuild();
        fx.invalidate(Provenance::Input);
    }

    fn activate(&mut self, key: u32, fx: &mut Effects<'_, InnerHost>) {
        let Some(row) = form_activate(&self.form, key, fx) else { return };
        if row != SeId::Forget {
            self.state.armed = false;
        }
        match row {
            SeId::Enabled => {
                let on = !self.state.enabled;
                self.send(SeerrChange::Enabled(on), fx);
                self.rebuild();
                fx.invalidate(Provenance::Input);
            }
            SeId::Address => self.begin_edit(fx),
            SeId::Test => {
                if self.state.url.is_empty() || self.state.probe == Probe::Testing || !self.writes.is_empty() {
                    return;
                }
                let (reply, receipt) = std::sync::mpsc::channel();
                fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::SeerrTest { reply })));
                self.test = Some(receipt);
                self.state.probe = Probe::Testing;
                self.state.problem = None;
                self.rebuild();
                fx.invalidate(Provenance::Input);
            }
            SeId::Forget => {
                if !self.state.armed {
                    self.state.armed = true;
                } else {
                    self.state.armed = false;
                    self.send(SeerrChange::Forget, fx);
                }
                self.rebuild();
                fx.invalidate(Provenance::Input);
            }
        }
    }

    /// Settle answered writes (followed by a read of what is now stored), a read and a test.
    fn poll(&mut self, fx: &mut Effects<'_, InnerHost>) -> bool {
        let mut moved = false;
        if !self.writes.is_empty() {
            self.writes.retain(|receipt| matches!(receipt.try_recv(), Err(TryRecvError::Empty)));
            if self.writes.is_empty() {
                self.ask_read(fx);
            }
        }
        if let Some(receipt) = &self.read {
            match receipt.try_recv() {
                Err(TryRecvError::Empty) => {}
                answer => {
                    self.read = None;
                    // a refused start leaves what the page already shows
                    if let Ok((url, enabled)) = answer {
                        if (url.as_str(), enabled) != (self.state.url.as_str(), self.state.enabled) {
                            self.state.probe = Probe::Idle;
                        }
                        (self.state.url, self.state.enabled) = (url, enabled);
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
                        Ok(Ok((status, me))) => Probe::Connected(status, me),
                        Ok(Err(error)) => Probe::Failed(error),
                        // the app could not start the request at all
                        _ => Probe::Failed(SeerrError::Unreachable),
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

impl Machine<InnerHost> for SeerrPage {
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
                // an address half typed is not sent
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
                if self.state.armed && self.state.sel != SeId::Forget.key() {
                    self.state.armed = false;
                    self.rebuild();
                }
                Handled::Yes
            }
            ScreenEvent::Activate(key) => {
                if let Some(edit) = self.state.edit.as_mut() {
                    if !edit.keyboard_up {
                        edit.keyboard_up = true;
                        Self::keyboard(true, fx);
                    }
                } else {
                    self.activate(*key, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::SystemKeyboard(up), .. }) => {
                // A system dismissal is also used on app background: it is not acceptance.
                if !*up && self.state.edit.is_some() {
                    self.finish_edit(false, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Text(edit), .. }) if self.state.edit.is_some() => {
                if let Some(typing) = self.state.edit.as_mut() {
                    typing.buf.edit_literal(edit);
                }
                self.state.problem = None;
                self.rebuild();
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key, sym, edge, .. }, .. })
                if self.state.edit.is_some() && *edge != plx_machine::machine::Edge::Up =>
            {
                if !self.state.edit.as_ref().unwrap().keyboard_up {
                    if *key == Key::Back {
                        self.finish_edit(false, fx);
                    } else if *key == Key::Ok {
                        self.state.edit.as_mut().unwrap().keyboard_up = true;
                        Self::keyboard(true, fx);
                    }
                    return Handled::Yes;
                }
                if char::from_u32(*sym).is_some_and(|c| !c.is_control()) {
                    return Handled::Yes;
                }
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
                // while the address is being typed every key is the keyboard's
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

plx_ui::focusable_via_view!(SeerrPage, InnerHost, view);

impl Screen<InnerHost> for SeerrPage {
    fn name(&self) -> &'static str { "seerr" }
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
#[path = "settings_seerr_tests.rs"]
mod tests;
