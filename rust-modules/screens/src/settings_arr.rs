//! **Settings > Integrations > Radarr** and **Sonarr**: where the viewer's Radarr (movies) or Sonarr
//! (shows) runs and the API key it answers to. They are used for one thing: after a title is deleted
//! from Plex the app asks the one that holds it to delete it too and to stop it coming back through
//! a list sync. Both are off until set, and each has its own page.
//!
//! Typed on the television's own keyboard, the address in the clear and the key as dots; a typed value
//! settles through [`ArrLink::normalize_url`] / [`ArrLink::normalize_key`] and is written as one
//! `PreferenceCmd::ArrLink`, which the app stores in the protected half of the session record.
//!
//! **The key never rests in this page.** The page knows only whether one is set; the draft lives only
//! while it is being typed, is shown as dots, enters no log line, and of the page's logical state only
//! its length is written.

use super::*;
use plx_machine::machine::{InputEvent, InputKind, TextEdit};
use plx_plex::plex::session::{ArrChange, ArrKind, ArrLink, UrlProblem};
use plx_platform::i18n::msg;
use plx_ui::text_buffer::TextBuffer;
use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ArId {
    Address,
    Key,
    Forget,
}

impl FormId for ArId {
    fn key(&self) -> RowKey {
        RowKey(match self {
            ArId::Address => 1,
            ArId::Key => 2,
            ArId::Forget => 3,
        })
    }
}

/// What a typed value was refused for; the status line says it until the next edit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Problem {
    Url(UrlProblem),
    Key,
}

/// The field being typed into, and what has been typed.
struct Edit {
    field: ArId,
    buf: TextBuffer,
    keyboard_up: bool,
}

/// The longest run of dots a masked draft shows: the row has a fixed width.
const MAX_DOTS: usize = 24;

/// The page's rows from plain inputs (the text-fit tests call this directly). `typing` is the
/// field being typed and its draft as the row shows it; `armed` the Forget row waiting for its
/// second OK.
fn arr_form(
    url: &str,
    key_set: bool,
    typing: Option<(ArId, &str)>,
    armed: bool,
    status: &str,
) -> Form<ArId, ArId, SettingsPage> {
    let address = match typing {
        Some((ArId::Address, draft)) => format!("{draft}|"),
        _ if url.is_empty() => msg::bazarr_not_set().to_string(),
        _ => url.to_string(),
    };
    let key = match typing {
        Some((ArId::Key, draft)) => format!("{}|", draft),
        _ if key_set => "\u{2022}".repeat(8),
        _ => msg::bazarr_not_set().to_string(),
    };
    let mut rows = FormSection::new("")
        .item(
            ArId::Address, RowKind::Button, ArId::Address,
            Row::new(msg::bazarr_address()).detail(msg::bazarr_address_detail()).value(address).chevron(true),
        )
        .item(
            ArId::Key, RowKind::Button, ArId::Key,
            Row::new(msg::bazarr_key()).detail(msg::bazarr_key_detail()).value(key).chevron(true),
        );
    if !url.is_empty() || key_set {
        let mut forget = Row::new(msg::bazarr_forget()).detail(msg::bazarr_forget_detail()).destructive(true);
        if armed {
            forget = forget.value(msg::bazarr_forget_sure());
        }
        rows = rows.item(ArId::Forget, RowKind::Button, ArId::Forget, forget);
    }
    Form::new().section(rows).section(FormSection::new("").note(status))
}

/// The status line: why a typed value was refused, or what the link is for.
fn status_text(set: bool, problem: Option<Problem>) -> String {
    match problem {
        Some(Problem::Url(UrlProblem::Scheme)) => msg::bazarr_problem_scheme().to_string(),
        Some(Problem::Url(UrlProblem::Plaintext)) => msg::bazarr_problem_plaintext().to_string(),
        Some(Problem::Url(_)) => msg::bazarr_problem_host().to_string(),
        Some(Problem::Key) => msg::bazarr_problem_key().to_string(),
        None if set => msg::arr_status_set().to_string(),
        None => msg::arr_status_unset().to_string(),
    }
}

/// The Settings root's value for a Radarr or Sonarr row: the host it is set to, or that it is not set up.
pub(super) fn summary(link: &ArrLink) -> String {
    match link.host() {
        Some(host) if link.is_complete() => host.to_string(),
        _ => msg::bazarr_summary_unset().to_string(),
    }
}

pub(super) struct ArrPage {
    kind: ArrKind,
    entry: EntryId,
    form: FormTable<ArId, ArId, SettingsPage>,
    state: ArState,
    /// A write sent and not yet answered: the rows show what was typed meanwhile.
    write: Option<Receiver<bool>>,
    /// A read of the stored link asked for and not yet answered (`PreferenceCmd::ArrRead`): sent
    /// when the page opens and again once a write is durable, so the rows only ever say what is stored.
    read: Option<Receiver<(String, bool)>>,
}

struct ArState {
    sel: RowKey,
    url: String,
    key_set: bool,
    edit: Option<Edit>,
    armed: bool,
    problem: Option<Problem>,
}

impl LogicalState for ArState {
    fn write(&self, w: &mut Canon) {
        // the key is never written, and of a draft only its length
        w.u32(self.sel.0).str(&self.url).bool(self.key_set).bool(self.armed);
        w.u32(self.edit.as_ref().map_or(0, |e| 1 + e.buf.text().len() as u32));
        w.u32(match self.problem {
            None => 0,
            Some(Problem::Key) => 1,
            Some(Problem::Url(_)) => 2,
        });
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!(
            "arr sel={} url_set={} key_set={} editing={}",
            self.sel.0, !self.url.is_empty(), self.key_set, self.edit.is_some()
        ));
    }
}

impl ArrPage {
    pub(super) fn new(kind: ArrKind, entry: EntryId) -> Self {
        let mut page = Self {
            kind,
            entry,
            form: FormTable::new(super::super::registry::BAND),
            state: ArState {
                sel: ArId::Address.key(),
                url: String::new(),
                key_set: false,
                edit: None,
                armed: false,
                problem: None,
            },
            write: None,
            read: None,
        };
        page.rebuild();
        page
    }

    fn rebuild(&mut self) {
        let keep = self.form.selected_id().copied().unwrap_or(ArId::Address);
        let draft = self.state.edit.as_ref().map(|e| {
            let text = if e.field == ArId::Key {
                "\u{2022}".repeat(e.buf.text().chars().count().min(MAX_DOTS))
            } else {
                e.buf.text().to_string()
            };
            (e.field, text)
        });
        let status = status_text(!self.state.url.is_empty() && self.state.key_set, self.state.problem);
        self.form.table.compact = false;
        let form = arr_form(
            &self.state.url, self.state.key_set,
            draft.as_ref().map(|(f, t)| (*f, t.as_str())), self.state.armed, &status,
        );
        self.form.set(form, Some(&keep));
        self.form.table.list_focused = true;
    }

    fn copy(&self) -> &'static str {
        match self.kind { ArrKind::Radarr => msg::arr_radarr_copy(), ArrKind::Sonarr => msg::arr_sonarr_copy() }
    }

    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(
            Header::new(RouteLayout::screen(), Some(msg::settings_title()), self.kind.name(), self.copy()),
            &self.form.table, GroupId(0), self.entry,
        ).keyed(&self.form)
    }

    /// Ask the app for the stored link; the answer lands in [`ArrPage::poll`].
    fn ask_read(&mut self, fx: &mut Effects<'_, InnerHost>) {
        let (reply, receipt) = std::sync::mpsc::channel();
        fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::ArrRead { kind: self.kind, reply })));
        self.read = Some(receipt);
    }

    fn keyboard(up: bool, fx: &mut Effects<'_, InnerHost>) {
        fx.push(Fx::Deliver(MachineId::Instance(InstanceId(0)), Delivery::Keyboard { up }));
    }

    /// Send one edit of the stored link and show it meanwhile.
    fn send(&mut self, change: ArrChange, fx: &mut Effects<'_, InnerHost>) {
        match &change {
            ArrChange::Url(url) => self.state.url = url.clone(),
            ArrChange::Key(_) => self.state.key_set = true,
            ArrChange::Forget => {
                self.state.url.clear();
                self.state.key_set = false;
            }
        }
        let (reply, receipt) = std::sync::mpsc::channel();
        fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::ArrLink { kind: self.kind, change, reply })));
        self.write = Some(receipt);
    }

    fn begin_edit(&mut self, field: ArId, fx: &mut Effects<'_, InnerHost>) {
        // a key is typed afresh: the stored one is not shown, and is not read here
        let text = if field == ArId::Address { self.state.url.clone() } else { String::new() };
        let len = text.len();
        self.state.edit = Some(Edit { field, buf: TextBuffer::new(text, len), keyboard_up: true });
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
            let result = match edit.field {
                ArId::Address => ArrLink::normalize_url(edit.buf.text())
                    .map(ArrChange::Url).map_err(Problem::Url),
                _ => ArrLink::normalize_key(edit.buf.text())
                    .map(ArrChange::Key).ok_or(Problem::Key),
            };
            match result {
                Ok(change) => {
                    self.state.problem = None;
                    self.send(change, fx);
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
        if row != ArId::Forget {
            self.state.armed = false;
        }
        match row {
            ArId::Address | ArId::Key => self.begin_edit(row, fx),
            ArId::Forget => {
                if !self.state.armed {
                    self.state.armed = true;
                } else {
                    self.state.armed = false;
                    self.send(ArrChange::Forget, fx);
                }
                self.rebuild();
                fx.invalidate(Provenance::Input);
            }
        }
    }

    /// Settle an answered write (which is followed by a read of what is now stored) and a read.
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
                        (self.state.url, self.state.key_set) = (url, key_set);
                    }
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

impl Machine<InnerHost> for ArrPage {
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
                if self.state.armed && self.state.sel != ArId::Forget.key() {
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

plx_ui::focusable_via_view!(ArrPage, InnerHost, view);

impl Screen<InnerHost> for ArrPage {
    fn name(&self) -> &'static str { "arr" }
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
#[path = "settings_arr_tests.rs"]
mod tests;
