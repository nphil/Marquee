//! Settings > Subtitles: the rows, each field's stepping, the write door, the preview's source and
//! the fit of every string in every shipped language.
use super::*;
use plx_base::fontcov::advances::ShippedMeasure;
use plx_machine::machine::Measure;
use plx_machine::present::Present;
use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
use plx_plex::plex::session::SubFont;

/// Holds the live look cell on `look` for the test and puts the default back after it.
struct Live;
impl Live {
    fn set(look: SubtitleLook) -> Self {
        plx_media::route::restore_subtitle_look(look);
        Live
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        plx_media::route::restore_subtitle_look(SubtitleLook::DEFAULT);
    }
}

fn run(page: &mut SubtitlesPage, f: impl FnOnce(&mut SubtitlesPage, &mut Effects<'_, InnerHost>)) -> Vec<Stamped<InnerHost>> {
    let mut out = Vec::new();
    let mut present = Present::new();
    let mut fx = Effects::new(&mut out, MachineId::Instance(InstanceId(0)), &mut present);
    f(page, &mut fx);
    out
}

fn write_request(effects: Vec<Stamped<InnerHost>>) -> Option<(SubtitleLook, std::sync::mpsc::Sender<bool>)> {
    let mut found = None;
    for event in effects {
        if let Fx::App(AppFx::Preferences(crate::registry::PreferenceCmd::Choose(
            plx_media::route::Choice::SubtitleLook(v), reply))) = event.fx {
            assert!(found.is_none(), "one pick submits one write");
            found = Some((v, reply));
        }
    }
    found
}

fn value(page: &SubtitlesPage, field: Field) -> Option<String> {
    let i = page.form.index_of_key(field.key()).expect("the row is in the form");
    page.form.table.sections.iter().flat_map(|s| s.rows.iter()).nth(i).and_then(|r| r.value.clone())
}

fn inputs() -> RootInputs {
    RootInputs {
        signed_in: true, multi_user: false, library_count: 0,
        auto_sign_in: false, trailer_autoplay: true, resolution: plx_plex::plex::session::UiResolution::Auto, resolution_offered: false,
        language: plx_platform::i18n::Preference::System,
        under_posters: plx_plex::plex::session::UnderPosters::DEFAULT, plaintext: Vec::new(),
        subtitle_preset: LookPreset::Marquee,
        bazarr: plx_plex::plex::session::BazarrLink::default(),
    }
}

#[test]
fn every_field_has_its_own_key_and_a_row() {
    let _guard = plx_base::testlock::serial();
    let _live = Live::set(SubtitleLook::DEFAULT);
    let page = SubtitlesPage::new(EntryId(0));
    let mut keys: Vec<u32> = Field::ALL.iter().map(|f| f.key().0).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), Field::ALL.len());
    assert!(keys.iter().all(|k| *k < crate::registry::BAND));
    for field in Field::ALL {
        assert!(page.form.index_of_key(field.key()).is_some(), "{field:?} has a row");
    }
    assert_eq!(keys_for_test().len(), 22);
}

#[test]
fn a_hand_edit_turns_the_preset_to_custom_and_a_preset_pick_keeps_the_ass_choice() {
    let look = LookPreset::Blend.look();
    let bolder = adjust(look, Field::Bold, 1, false);
    assert!(bolder.bold && bolder.preset == LookPreset::Custom);
    // a change that changes nothing keeps the label
    assert_eq!(adjust(SubtitleLook { opacity: 100, ..look }, Field::Opacity, 1, false).preset, look.preset);
    let mine = SubtitleLook { ass: AssMode::Mine, ..LookPreset::Classic.look() };
    let next = adjust(mine, Field::Preset, 1, false);
    assert_eq!(next.preset, LookPreset::Netflix);
    assert_eq!(next.ass, AssMode::Mine);
}

#[test]
fn the_preset_row_steps_through_the_picker_and_custom_enters_at_either_end() {
    let mut look = LookPreset::Blend.look();
    let mut seen = vec![look.preset];
    for _ in 0..LookPreset::PICKER.len() - 1 {
        look = adjust(look, Field::Preset, 1, false);
        seen.push(look.preset);
    }
    assert_eq!(seen, LookPreset::PICKER.to_vec(), "RIGHT walks the picker in order");
    assert_eq!(adjust(look, Field::Preset, 1, false).preset, LookPreset::Blend, "and wraps");
    let custom = adjust(LookPreset::Blend.look(), Field::Bold, 1, false);
    assert_eq!(adjust(custom, Field::Preset, 1, false).preset, LookPreset::PICKER[0]);
    assert_eq!(adjust(custom, Field::Preset, -1, false).preset, *LookPreset::PICKER.last().unwrap());
}

#[test]
fn numeric_rows_clamp_with_the_arrows_and_wrap_with_ok() {
    let top = SubtitleLook { opacity: 100, edge_w: 8, line_pct: 200, nudge: 400, back_radius: 40, ..SubtitleLook::DEFAULT };
    for field in [Field::Opacity, Field::EdgeWidth, Field::LineSpacing, Field::Nudge, Field::BackRadius] {
        assert_eq!(adjust(top, field, 1, false), top, "{field:?} stops at its top");
        assert_ne!(adjust(top, field, 1, true), top, "{field:?} wraps on OK");
        assert_ne!(adjust(top, field, -1, false), top, "{field:?} steps down");
    }
    let low = SubtitleLook { opacity: 10, edge_w: 1, line_pct: 70, nudge: -60, back_pad: 0, ..SubtitleLook::DEFAULT };
    for field in [Field::Opacity, Field::EdgeWidth, Field::LineSpacing, Field::Nudge, Field::BackPad] {
        assert_eq!(adjust(low, field, -1, false), low, "{field:?} stops at its bottom");
    }
    // the exact values the rows step by
    assert_eq!(adjust(SubtitleLook::DEFAULT, Field::Opacity, -1, false).opacity, 95);
    assert_eq!(adjust(SubtitleLook::DEFAULT, Field::Nudge, 1, false).nudge, 10);
    assert_eq!(adjust(SubtitleLook::DEFAULT, Field::LineSpacing, 1, false).line_pct, 105);
    assert_eq!(adjust(SubtitleLook::DEFAULT, Field::Size, 1, false).size_pct, 110, "Automatic reads as 100");
}

#[test]
fn colour_rows_go_from_automatic_through_the_palette_and_take_a_presets_own_colour() {
    let mut look = SubtitleLook::DEFAULT;
    let mut seen = vec![look.color];
    for _ in 0..COLORS.len() {
        look = adjust(look, Field::Color, 1, false);
        seen.push(look.color);
    }
    assert_eq!(seen[0], None);
    assert_eq!(seen[1..].iter().map(|c| c.unwrap()).collect::<Vec<_>>(), COLORS.to_vec());
    assert_eq!(adjust(look, Field::Color, 1, false).color, None, "and wraps to Automatic");
    let blend = LookPreset::Blend.look();
    assert!(!COLORS.contains(&blend.color.unwrap()), "the Blend preset's colour is its own");
    assert_eq!(adjust(blend, Field::Color, 1, false).color, None, "forward from it is Automatic");
    assert_eq!(adjust(blend, Field::Color, -1, false).color, Some(*COLORS.last().unwrap()));
    let back = adjust(SubtitleLook::DEFAULT, Field::BackColor, 1, false);
    assert_eq!(back.back_color, BACK_COLORS[1]);
}

#[test]
fn reset_returns_to_the_named_preset_or_the_apps_own_when_there_is_none() {
    let edited = adjust(LookPreset::Classic.look(), Field::Font, 1, false);
    assert_eq!(edited.preset, LookPreset::Custom);
    let reset = adjust(LookPreset::Netflix.look(), Field::Bold, 1, false);
    assert_eq!(adjust(reset, Field::Reset, 1, true), LookPreset::Marquee.look());
    let mut look = LookPreset::HighContrast.look();
    look.back_opacity = 10;
    assert_eq!(adjust(SubtitleLook { preset: LookPreset::HighContrast, ..look }, Field::Reset, 1, true), LookPreset::HighContrast.look());
}

#[test]
fn rows_that_mean_nothing_are_dimmed_and_do_nothing() {
    let _guard = plx_base::testlock::serial();
    let bare = SubtitleLook { edge: SubEdge::Raised, back: SubBack::Off, ..SubtitleLook::DEFAULT };
    let _live = Live::set(bare);
    let mut page = SubtitlesPage::new(EntryId(0));
    for field in [Field::EdgeWidth, Field::BackOpacity, Field::BackRadius, Field::BackPad, Field::BackColor] {
        assert!(!applies(&bare, field), "{field:?}");
        let out = run(&mut page, |p, fx| p.adjust_row(field.key().0, 1, true, fx));
        assert!(write_request(out).is_none(), "{field:?} writes nothing");
    }
    assert!(applies(&SubtitleLook { back: SubBack::Line, ..bare }, Field::BackPad));
}

#[test]
fn the_rows_read_the_look_in_words() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let look = SubtitleLook { size_pct: 130, opacity: 90, edge_w: 3, nudge: 120, line_pct: 105, ..LookPreset::Blend.look() };
    let _live = Live::set(look);
    let page = SubtitlesPage::new(EntryId(0));
    let read = |f| value(&page, f);
    assert_eq!(read(Field::Preset).as_deref(), Some("Blend with video"));
    assert_eq!(read(Field::Size).as_deref(), Some("130%"));
    assert_eq!(read(Field::Opacity).as_deref(), Some("90%"));
    assert_eq!(read(Field::EdgeWidth).as_deref(), Some("3 px"));
    assert_eq!(read(Field::Nudge).as_deref(), Some("120 px"));
    assert_eq!(read(Field::Color).as_deref(), Some("#EDEBF4"), "a colour off the palette reads as its number");
    assert_eq!(read(Field::Font).as_deref(), Some("Default"));
    assert_eq!(read(Field::Anchor).as_deref(), Some("Bottom"));
    assert_eq!(read(Field::Styled).as_deref(), Some("Keep"));
    assert_eq!(read(Field::BackRadius).as_deref(), Some("8 px"));
    assert_eq!(read(Field::Bold), None, "a switch states itself as a toggle");
    let auto = SubtitlesPage::new(EntryId(0));
    assert_eq!(value(&auto, Field::Size).as_deref(), Some("130%"));
    assert_eq!(value_of(&SubtitleLook::DEFAULT, Choices { sample: 0, scene: 0 }, Field::Size).as_deref(), Some("Automatic"));
    assert_eq!(value_of(&SubtitleLook { back_radius: 0, ..SubtitleLook::DEFAULT }, Choices { sample: 0, scene: 0 }, Field::BackRadius).as_deref(), Some("Square"));
}

#[test]
fn a_pick_goes_through_the_write_door_and_shows_at_once() {
    let _guard = plx_base::testlock::serial();
    let _live = Live::set(SubtitleLook::DEFAULT);
    let mut page = SubtitlesPage::new(EntryId(0));
    let before = page.state.hash();
    let out = run(&mut page, |p, fx| p.adjust_row(Field::Font.key().0, 1, false, fx));
    let (asked, reply) = write_request(out).expect("a look write");
    assert_eq!(asked.font, SubFont::Serif);
    assert_eq!(asked.preset, LookPreset::Custom);
    assert_eq!(page.state.look, asked, "the row shows the pick before the write answers");
    assert_ne!(page.state.hash(), before, "the look belongs to replay state");
    assert!(!page.poll(), "an unanswered write leaves the page alone");
    plx_media::route::restore_subtitle_look(asked);
    reply.send(true).unwrap();
    assert!(!page.poll(), "an answer that agrees changes nothing");
    assert_eq!(page.state.look, asked);
}

#[test]
fn the_page_follows_a_look_the_players_style_pages_changed() {
    let _guard = plx_base::testlock::serial();
    let _live = Live::set(SubtitleLook::DEFAULT);
    let mut page = SubtitlesPage::new(EntryId(0));
    let theirs = LookPreset::Netflix.look();
    plx_media::route::restore_subtitle_look(theirs);
    assert!(page.poll());
    assert_eq!(page.state.look, theirs);
    assert_eq!(value(&page, Field::Preset).as_deref(), Some(preset_label(LookPreset::Netflix)));
}

#[test]
fn ok_cycles_a_row_and_left_or_right_never_resets() {
    let _guard = plx_base::testlock::serial();
    let _live = Live::set(LookPreset::Classic.look());
    let mut page = SubtitlesPage::new(EntryId(0));
    assert!(write_request(run(&mut page, |p, fx| p.adjust_row(Field::Reset.key().0, 1, false, fx))).is_none(), "an arrow on Reset does nothing");
    let (reset, _) = write_request(run(&mut page, |p, fx| p.adjust_row(Field::Bold.key().0, 1, true, fx))).expect("OK on Bold toggles it");
    assert!(reset.bold);
    let (back, _) = write_request(run(&mut page, |p, fx| p.adjust_row(Field::Reset.key().0, 1, true, fx))).expect("OK on Reset resets");
    assert_eq!(back, LookPreset::Marquee.look(), "the look was Custom after the edit");
}

#[test]
fn the_sample_and_scene_rows_are_the_pages_own_and_write_nothing() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let _live = Live::set(SubtitleLook::DEFAULT);
    let mut page = SubtitlesPage::new(EntryId(0));
    let out = run(&mut page, |p, fx| {
        p.adjust_row(Field::Sample.key().0, 1, false, fx);
        p.adjust_row(Field::Scene.key().0, -1, false, fx);
    });
    assert!(write_request(out).is_none());
    assert_eq!(page.state.choices, Choices { sample: 1, scene: SCENES.len() - 1 });
    assert_eq!(value(&page, Field::Sample).as_deref(), Some("CJK"));
    assert_eq!(value(&page, Field::Scene).as_deref(), Some("Snow"));
    assert!(page.tag.to_str().unwrap().starts_with("Sample 2 of 6"), "{:?}", page.tag);
}

#[test]
fn the_sample_cues_become_ass_in_the_players_own_style() {
    let style = TextStyle::default();
    for sample in 0..SAMPLE_COUNT {
        let source = SampleSource::build(sample, 1, 1, &style);
        let ass::Content::Script { bytes, .. } = &source.content else { panic!("a script") };
        let text = String::from_utf8_lossy(bytes);
        assert!(text.contains("Dialogue:"), "sample {sample} has a cue");
        assert_eq!(source.origin.is_text(), sample < SAMPLES.len(), "only the authored sample keeps its own look");
    }
    let accents = SampleSource::build(0, 1, 1, &style);
    let ass::Content::Script { bytes, .. } = &accents.content else { panic!("a script") };
    assert!(String::from_utf8_lossy(bytes).contains("smörgåsbord"));
    // a restyle keeps the timeline and moves the revision
    let mut preview = Preview::new();
    let look = SubtitleLook::DEFAULT;
    preview.tick(10, &look, Choices { sample: 0, scene: 0 });
    let (id, revision) = preview.source.as_ref().map(|s| (s.id, s.revision)).unwrap();
    preview.tick(20, &SubtitleLook { font: SubFont::Serif, ..look }, Choices { sample: 0, scene: 0 });
    let held = preview.source.as_ref().unwrap();
    assert_eq!((held.id, held.revision), (id, revision + 1));
    preview.tick(30, &SubtitleLook { font: SubFont::Serif, opacity: 50, color: Some(0xFFE680), ..look }, Choices { sample: 0, scene: 0 });
    assert_eq!(preview.source.as_ref().unwrap().revision, revision + 1, "a colour is painted, not restyled");
    preview.tick(40, &look, Choices { sample: 1, scene: 0 });
    assert_ne!(preview.source.as_ref().unwrap().id, id, "another sample is another timeline");
}

#[test]
fn the_scenes_are_bright_dark_and_nearly_white() {
    assert!(SCENES[1].luma < 0.15 && SCENES[0].luma > 0.4 && SCENES[2].luma > 0.9);
}

#[test]
fn the_preview_rests_on_the_narrative_floor_inside_the_screen() {
    let layout = page_layout();
    let frame = preview_frame(layout);
    assert!((frame.w / frame.h - 16.0 / 9.0).abs() < 0.01, "a 16:9 picture");
    assert!(frame.x + frame.w <= layout.content.x, "clear of the rows");
    assert!(frame.y + frame.h <= layout.narrative.y + layout.narrative.h + 0.5);
}

#[test]
fn the_root_row_opens_the_page_and_reads_the_preset() {
    let _guard = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let mut form = FormTable::<RootId, Action, SettingsPage>::new(crate::registry::BAND);
    form.table.compact = false;
    form.set(root_form(&RootInputs { subtitle_preset: LookPreset::Netflix, ..inputs() }), None);
    let i = form.index_of_key(RootId::Subtitles.key()).expect("the row is on the root");
    assert_eq!(form.activate(i), Some(plx_ui::form::Activation::Push(SettingsPage::Subtitles)));
    let row = form.table.sections.iter().flat_map(|s| s.rows.iter()).nth(i).unwrap();
    assert_eq!(row.value.as_deref(), Some("Netflix"));
}

/// **Every string fits, in every shipped language**: the rows beside a spread of looks (each value
/// a row can read) and the narrative copy of every preset clear of the preview.
#[test]
fn every_subtitles_string_fits_in_every_language() {
    use plx_ui::text_view::TextView;
    let _guard = plx_base::testlock::serial();
    let layout = page_layout();
    let mut looks: Vec<SubtitleLook> = LookPreset::PICKER.iter().map(|p| p.look()).collect();
    looks.push(SubtitleLook { size_pct: 130, color: Some(COLORS[2]), back: SubBack::Window, back_color: BACK_COLORS[3], back_radius: 0, ass: AssMode::Mine, reveal: Reveal::Line, anchor: SubAnchor::Top, ..LookPreset::HighContrast.look() });
    looks.push(SubtitleLook { font: SubFont::Legible, edge: SubEdge::Depressed, reveal: Reveal::Word, preset: LookPreset::Custom, ..SubtitleLook::DEFAULT });
    let mut out = Vec::new();
    for language in SHIPPED {
        let _lang = language_on_this_thread_for_test(language);
        let tag = language.tag();
        for look in &looks {
            for sample in 0..SAMPLE_COUNT {
                let mut page = FormTable::<Field, Field, SettingsPage>::new(crate::registry::BAND);
                page.table.compact = false;
                page.set(subtitles_form(look, Choices { sample, scene: sample % SCENES.len() }), None);
                out.extend(page.table.app_fit_failures(layout.sectioned_table().w, &format!("{tag} subtitles {:?} {sample}", look.preset)));
            }
        }
        for preset in LookPreset::PICKER.iter().chain([&LookPreset::Custom]) {
            let title = msg::subtitles_title();
            let frame = layout.narrative_copy_frame(true, title, layout.action.y, &ShippedMeasure);
            let copy_h = TextView::new(about(*preset), theme::size::LABEL, theme::TEXT_READING)
                .leading(theme::size::LABEL as f32 + theme::space::XS)
                .max_lines(12)
                .with_measure(&ShippedMeasure)
                .measure_h(layout.narrative.w);
            if frame.y + copy_h + theme::space::MD > preview_frame(layout).y {
                out.push(format!("{tag}: the copy about {preset:?} ends at {:.0}, under the preview at {:.0}", frame.y + copy_h, preview_frame(layout).y));
            }
        }
        let c = Choices { sample: 0, scene: 0 };
        let tagline = msg::subtitles_sample_tag(sample_name(c.sample), 1, SAMPLE_COUNT as i64);
        if ShippedMeasure.width_str(&tagline, theme::size::CAPTION, false) + 4.0 * theme::space::MD > PREVIEW_W {
            out.push(format!("{tag}: the sample tag {tagline:?} is wider than the preview"));
        }
    }
    plx_ui::table::assert_no_fit_failures(&out);
}
