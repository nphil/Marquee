//! Settings > Integrations > SpeechDock, using the television's existing text editor.
use super::*;
use plx_machine::machine::{InputEvent, InputKind, TextEdit};
use plx_platform::i18n::msg;
use plx_ui::text_buffer::TextBuffer;
use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Id { Address }
impl FormId for Id {
    fn key(&self) -> RowKey { RowKey(1) }
}

struct Edit { buf: TextBuffer, keyboard_up: bool }
struct State { sel: RowKey, edit: Option<Edit>, error: Option<Error> }
#[derive(Clone, Copy)]
enum Error { Invalid, Save }
impl LogicalState for State {
    fn write(&self, w: &mut Canon) {
        w.u32(self.sel.0).u32(self.edit.as_ref().map_or(0, |e| 1 + e.buf.text().len() as u32));
        w.u32(match self.error { None => 0, Some(Error::Invalid) => 1, Some(Error::Save) => 2 });
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("speechdock sel={} editing={}", self.sel.0, self.edit.is_some()));
    }
}

pub(super) struct SpeechDockPage {
    entry: EntryId,
    form: FormTable<Id, Id, SettingsPage>,
    state: State,
    write: Option<Receiver<bool>>,
}
impl SpeechDockPage {
    pub(super) fn new(entry: EntryId) -> Self {
        let mut page = Self { entry, form: FormTable::new(super::super::registry::BAND),
            state: State { sel: Id::Address.key(), edit: None, error: None }, write: None };
        page.rebuild();
        page
    }
    fn rebuild(&mut self) {
        let address = match &self.state.edit {
            Some(edit) => format!("{}|", edit.buf.text()),
            None => plx_media::player::speechdock_settings::origin().unwrap_or_else(|| msg::speechdock_disabled().to_string()),
        };
        let rows = FormSection::new("").item(Id::Address, RowKind::Button, Id::Address,
            Row::new(msg::bazarr_address()).detail(msg::speechdock_address_detail()).value(address).chevron(true));
        let mut form = Form::new().section(rows);
        if let Some(error) = self.state.error {
            form = form.section(FormSection::new("").note(match error {
                Error::Invalid => msg::speechdock_invalid(),
                Error::Save => msg::speechdock_save_failed(),
            }));
        }
        self.form.table.compact = false;
        self.form.set(form, Some(&Id::Address));
        self.form.table.list_focused = true;
    }
    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(Header::new(RouteLayout::screen(), Some(msg::settings_title()),
            msg::speechdock_title(), msg::speechdock_copy()), &self.form.table, GroupId(0), self.entry).keyed(&self.form)
    }
    fn keyboard(up: bool, fx: &mut Effects<'_, InnerHost>) {
        fx.push(Fx::Deliver(MachineId::Instance(InstanceId(0)), Delivery::Keyboard { up }));
    }
    fn begin_edit(&mut self, fx: &mut Effects<'_, InnerHost>) {
        if self.write.is_some() { return; }
        let text = plx_media::player::speechdock_settings::origin().unwrap_or_default();
        let len = text.len();
        self.state.edit = Some(Edit { buf: TextBuffer::new(text, len), keyboard_up: true });
        self.state.error = None;
        Self::keyboard(true, fx);
        self.rebuild();
        fx.invalidate(Provenance::Input);
    }
    fn finish_edit(&mut self, commit: bool, fx: &mut Effects<'_, InnerHost>) {
        let Some(mut edit) = self.state.edit.take() else { return };
        if edit.keyboard_up { Self::keyboard(false, fx); }
        edit.keyboard_up = false;
        if commit {
            match plx_media::player::speechdock_settings::normalize_origin(edit.buf.text()) {
                Ok(origin) => {
                    let (reply, receipt) = std::sync::mpsc::channel();
                    fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::SpeechDockOrigin {
                        origin: origin.unwrap_or_default(), reply,
                    })));
                    self.write = Some(receipt);
                    self.state.error = None;
                }
                Err(_) => { self.state.error = Some(Error::Invalid); self.state.edit = Some(edit); }
            }
        }
        self.rebuild();
        fx.invalidate(Provenance::Input);
    }
    fn activate(&mut self, key: u32, fx: &mut Effects<'_, InnerHost>) {
        if form_activate(&self.form, key, fx).is_some() { self.begin_edit(fx); }
    }
}
impl Machine<InnerHost> for SpeechDockPage {
    type Ev = ScreenEvent<InnerHost>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, InnerHost>, fx: &mut Effects<'_, InnerHost>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                fx.push(Fx::Deliver(MachineId::Instance(InstanceId(0)), Delivery::Screen(ScreenEvent::Enter(
                    Enter::Fresh { focus: FocusTarget::Elem(FocusKey { entry: self.entry, elem: self.state.sel.0 }) }))));
                Handled::Yes
            }
            ScreenEvent::WillLeave(_) | ScreenEvent::Unmount | ScreenEvent::Cover | ScreenEvent::Suspend => {
                if self.state.edit.take().is_some() { Self::keyboard(false, fx); self.rebuild(); }
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                if let Some(receipt) = &self.write {
                    match receipt.try_recv() {
                        Ok(saved) => { self.write = None; self.state.error = if saved { None } else { Some(Error::Save) }; self.rebuild(); fx.invalidate(Provenance::Input); }
                        Err(TryRecvError::Disconnected) => { self.write = None; self.state.error = Some(Error::Save); self.rebuild(); fx.invalidate(Provenance::Input); }
                        Err(TryRecvError::Empty) => {}
                    }
                }
                self.form.table.update(t.dt(), RouteLayout::screen().sectioned_table().h);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                form_focus(&mut self.form, to.elem);
                if let Some(key) = self.form.key_at(self.form.table.sel.max(0) as usize) { self.state.sel = key; }
                Handled::Yes
            }
            ScreenEvent::Activate(key) => {
                if let Some(edit) = self.state.edit.as_mut() {
                    if !edit.keyboard_up { edit.keyboard_up = true; Self::keyboard(true, fx); }
                } else { self.activate(*key, fx); }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::SystemKeyboard(up), .. }) => {
                if !*up && self.state.edit.is_some() { self.finish_edit(false, fx); }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Text(edit), .. }) if self.state.edit.is_some() => {
                self.state.edit.as_mut().unwrap().buf.edit_literal(edit);
                self.state.error = None; self.rebuild(); fx.invalidate(Provenance::Input); Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key, sym, edge, .. }, .. })
                if self.state.edit.is_some() && *edge != plx_machine::machine::Edge::Up => {
                if !self.state.edit.as_ref().unwrap().keyboard_up {
                    if *key == Key::Back { self.finish_edit(false, fx); }
                    else if *key == Key::Ok { self.state.edit.as_mut().unwrap().keyboard_up = true; Self::keyboard(true, fx); }
                    return Handled::Yes;
                }
                if char::from_u32(*sym).is_some_and(|c| !c.is_control()) { return Handled::Yes; }
                let caret = match (*key, *sym) {
                    (Key::Left, _) => Some(TextEdit::Left), (Key::Right, _) => Some(TextEdit::Right),
                    (_, plx_ui::consts::SDLK_BACKSPACE) => Some(TextEdit::Backspace),
                    (_, plx_ui::consts::SDLK_CLEAR) => Some(TextEdit::Clear), _ => None,
                };
                if let Some(edit) = caret {
                    self.state.edit.as_mut().unwrap().buf.edit(&edit);
                    self.rebuild(); fx.invalidate(Provenance::Input);
                } else if *key == Key::Back { self.finish_edit(false, fx); }
                else if *key == Key::Ok { self.finish_edit(true, fx); }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Right, at_edge: true, .. }, .. }) => {
                if let Some(key) = cx.focus.current.and_then(|k| form_right_target(&self.form, k.elem)) { self.activate(key, fx); }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}
plx_ui::focusable_via_view!(SpeechDockPage, InnerHost, view);
impl Screen<InnerHost> for SpeechDockPage {
    fn name(&self) -> &'static str { "speechdock" }
    fn state(&self) -> &dyn LogicalState { &self.state }
    fn crumb(&self, _cx: &Cx<'_, InnerHost>) -> Option<Cow<'_, str>> { Some(Cow::Borrowed(msg::settings_title())) }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, InnerHost>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, InnerHost>) { plx_ui::screen::Part::<InnerHost>::draw(&mut self.view(), f, Rect::FULL); }
    fn render(&self) -> RenderStrategy { RenderStrategy::Page }
    fn focus_source(&self) -> FocusSource { FocusSource::Engine }
    fn hit_source(&self) -> HitSource { HitSource::Engine }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_machine::present::Present;

    fn settle(text: &str, commit: bool) -> (SpeechDockPage, Vec<crate::registry::PreferenceCmd>) {
        let mut page = SpeechDockPage::new(EntryId(0));
        page.state.edit = Some(Edit { buf: TextBuffer::new(text.into(), text.len()), keyboard_up: true });
        let mut out = Vec::new();
        let mut present = Present::new();
        let mut fx = Effects::new(&mut out, MachineId::Instance(InstanceId(0)), &mut present);
        page.finish_edit(commit, &mut fx);
        let commands = out.into_iter().filter_map(|event| match event.fx {
            Fx::App(AppFx::Preferences(command)) => Some(command), _ => None,
        }).collect();
        (page, commands)
    }

    #[test]
    fn editor_saves_only_origins_and_empty_disables() {
        let _guard = plx_base::testlock::serial();
        for (input, expected) in [("https://dock.example:8443/", "https://dock.example:8443"), (" ", "")] {
            let (_, commands) = settle(input, true);
            assert!(matches!(commands.as_slice(), [crate::registry::PreferenceCmd::SpeechDockOrigin { origin, .. }] if origin == expected));
        }
        for input in ["https://user:pass@dock.example", "https://dock.example/path", "https://dock.example?x=1"] {
            let (page, commands) = settle(input, true);
            assert!(commands.is_empty());
            assert!(matches!(page.state.error, Some(Error::Invalid)));
            assert_eq!(page.state.edit.unwrap().buf.text(), input);
        }
        let (_, commands) = settle("https://dock.example", false);
        assert!(commands.is_empty());
    }
}
