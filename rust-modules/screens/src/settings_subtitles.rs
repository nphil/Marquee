//! **Settings > Subtitles**: every field of the viewer's subtitle look as one row, beside a live
//! preview of what the look draws.
//!
//! The look is one stored value per profile ([`SubtitleLook`]); a row steps ONE field of it with
//! LEFT and RIGHT (OK steps forward and wraps), and the pick goes through the one write door every
//! live setting uses (`PreferenceCmd::Choose`, Optimistic), so the player's next cue wears it at
//! once. A hand edit turns the preset label to Custom; the Preset row steps through the presets,
//! and Reset returns to the look the label names. The player's Style pages edit the same value.
//!
//! **The preview is the real renderer** ([`AssSubtitles`]): the sample lines are converted to ASS
//! with the player's own converter and header (`text_ass`), libass draws them for the whole logical
//! screen, and the frame is scaled into the picture, so size, line pitch, placement, edge, box and
//! the Blend tone are the player's arithmetic and not a lookalike. The look it draws is the one the
//! rows show: a pick is on the picture in the frame it is made, before the write answers. The picture
//! is a plain gradient standing for a scene (daylight, night, snow): the Blend tone follows its
//! luma, and a bright scene shows why a caption needs an edge.
//!
//! With Reveal as spoken on, the sample cue replays in a loop so the reveal can be watched.

use super::*;
use crate::registry::AssSubtitles;
use plx_appkit::track_menu::{back_label, edge_label, font_label, preset_label, reveal_label};
use plx_media::player::text_style::TextStyle;
use plx_media::player::video_geometry::Viewport;
use plx_media::player::{ass, text_ass};
use plx_plex::plex::session::{
    AssMode, LookPreset, Reveal, SubAnchor, SubBack, SubEdge, SubtitleLook, BACK_COLORS, COLORS,
};
use plx_platform::i18n::msg;
use std::sync::Arc;

// ---------------------------------------------------------------------------------------------
// the fields
// ---------------------------------------------------------------------------------------------

/// One row. The same value is the row's identity, its action and what a test addresses it by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Preset,
    Sample,
    Scene,
    Font,
    Size,
    Color,
    Opacity,
    Bold,
    Edge,
    EdgeWidth,
    Back,
    BackOpacity,
    BackRadius,
    BackPad,
    BackColor,
    Anchor,
    Nudge,
    LineSpacing,
    Blend,
    Reveal,
    Styled,
    Reset,
}

impl Field {
    const ALL: [Field; 22] = [
        Field::Preset, Field::Sample, Field::Scene,
        Field::Font, Field::Size, Field::Color, Field::Opacity, Field::Bold,
        Field::Edge, Field::EdgeWidth,
        Field::Back, Field::BackOpacity, Field::BackRadius, Field::BackPad, Field::BackColor,
        Field::Anchor, Field::Nudge, Field::LineSpacing,
        Field::Blend, Field::Reveal, Field::Styled, Field::Reset,
    ];

    fn index(self) -> u32 {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(0) as u32
    }
}

impl FormId for Field {
    fn key(&self) -> RowKey {
        RowKey(1 + self.index())
    }
}

/// A look stepped by one field, or `look` itself when the step changes nothing.
fn edit(look: SubtitleLook, change: impl FnOnce(&mut SubtitleLook)) -> SubtitleLook {
    let next = look.edited(change);
    if (SubtitleLook { preset: look.preset, ..next }) == look { look } else { next }
}

/// `value` moved `delta` steps of `step` inside `lo..=hi`: clamped, or wrapped from one end to the
/// other when `wrap` (what OK does, so one key cycles a row).
fn ladder(value: i32, step: i32, lo: i32, hi: i32, delta: i32, wrap: bool) -> i32 {
    let next = value + step * delta;
    if next > hi && wrap && value == hi {
        lo
    } else if next < lo && wrap && value == lo {
        hi
    } else {
        next.clamp(lo, hi)
    }
}

/// The index of `value` in `list` stepped `delta` places on, wrapping. A value not on the list
/// (a preset's own colour) lands on the first entry going forward and the last going back.
fn cycle<T: PartialEq>(list: &[T], value: &T, delta: i32) -> usize {
    let n = list.len() as i32;
    match list.iter().position(|v| v == value) {
        Some(i) => (i as i32 + delta).rem_euclid(n) as usize,
        None if delta >= 0 => 0,
        None => (n - 1) as usize,
    }
}

/// Where a field's effect shows: Edge width means nothing to a bevel or a bare face, and the box rows
/// mean nothing without a box.
fn applies(look: &SubtitleLook, field: Field) -> bool {
    match field {
        Field::EdgeWidth => matches!(look.edge, SubEdge::Outline | SubEdge::Drop | SubEdge::Soft),
        Field::BackOpacity | Field::BackRadius | Field::BackPad | Field::BackColor => look.back != SubBack::Off,
        _ => true,
    }
}

/// The look after `field` is stepped `delta` places (`wrap`: OK's cycling rather than LEFT/RIGHT's
/// clamping). The Sample and Scene rows are the page's own and change nothing here.
fn adjust(look: SubtitleLook, field: Field, delta: i32, wrap: bool) -> SubtitleLook {
    let n = |v: u8, step: i32, lo: i32, hi: i32| ladder(i32::from(v), step, lo, hi, delta, wrap);
    match field {
        Field::Preset => {
            let next = cycle(&LookPreset::PICKER, &look.preset, delta);
            LookPreset::PICKER[next].applied(&look)
        }
        Field::Sample | Field::Scene => look,
        Field::Font => edit(look, |l| l.font = l.font.step(delta)),
        Field::Size => edit(look, |l| l.size_pct = SubtitleLook::size_step(l.size_pct, delta)),
        Field::Color => {
            // Automatic (the player's Color ladder) first, then the palette
            let mut options = vec![None];
            options.extend(COLORS.iter().map(|c| Some(*c)));
            let next = options[cycle(&options, &look.color, delta)];
            edit(look, |l| l.color = next)
        }
        Field::Opacity => edit(look, |l| l.opacity = n(l.opacity, 5, 10, 100) as u8),
        Field::Bold => edit(look, |l| l.bold = !l.bold),
        Field::Edge => edit(look, |l| l.edge = l.edge.step(delta)),
        Field::EdgeWidth => edit(look, |l| l.edge_w = n(l.edge_w, 1, 1, 8) as u8),
        Field::Back => edit(look, |l| l.back = l.back.step(delta)),
        Field::BackOpacity => edit(look, |l| l.back_opacity = n(l.back_opacity, 5, 0, 100) as u8),
        Field::BackRadius => edit(look, |l| l.back_radius = n(l.back_radius, 2, 0, 40) as u8),
        Field::BackPad => edit(look, |l| l.back_pad = n(l.back_pad, 2, 0, 40) as u8),
        Field::BackColor => {
            let next = BACK_COLORS[cycle(&BACK_COLORS, &look.back_color, delta)];
            edit(look, |l| l.back_color = next)
        }
        Field::Anchor => edit(look, |l| l.anchor = l.anchor.step(delta)),
        Field::Nudge => edit(look, |l| l.nudge = ladder(i32::from(l.nudge), 10, -60, 400, delta, wrap) as i16),
        Field::LineSpacing => edit(look, |l| l.line_pct = n(l.line_pct, 5, 70, 200) as u8),
        Field::Blend => edit(look, |l| l.blend = !l.blend),
        Field::Reveal => edit(look, |l| l.reveal = l.reveal.step(delta)),
        Field::Styled => edit(look, |l| l.ass = l.ass.step(delta)),
        // a hand-edited look has no preset to return to: it returns to the app's own
        Field::Reset => match look.preset {
            LookPreset::Custom => LookPreset::Marquee.applied(&look),
            preset => preset.applied(&look),
        },
    }
}

// ---------------------------------------------------------------------------------------------
// the words
// ---------------------------------------------------------------------------------------------

fn color_name(color: u32, palette: &[u32], names: &[&'static str]) -> String {
    palette
        .iter()
        .position(|c| *c == color)
        .map_or_else(|| format!("#{color:06X}"), |i| names[i].to_string())
}

fn text_color(color: Option<u32>) -> String {
    let names = [
        msg::subtitles_color_white(), msg::subtitles_color_soft_white(), msg::subtitles_color_yellow(),
        msg::subtitles_color_cyan(), msg::subtitles_color_green(), msg::subtitles_color_pink(),
    ];
    color.map_or_else(|| msg::subtitles_value_auto().to_string(), |c| color_name(c, &COLORS, &names))
}

fn box_color(color: u32) -> String {
    let names = [
        msg::subtitles_color_black(), msg::subtitles_color_plum(), msg::subtitles_color_slate(), msg::subtitles_color_brown(),
    ];
    color_name(color, &BACK_COLORS, &names)
}

fn percent(v: impl std::fmt::Display) -> String {
    format!("{v}%")
}

fn pixels(v: impl std::fmt::Display) -> String {
    format!("{v} px")
}

fn anchor_label(anchor: SubAnchor) -> &'static str {
    match anchor {
        SubAnchor::Bottom => msg::subtitles_anchor_bottom(),
        SubAnchor::Top => msg::subtitles_anchor_top(),
    }
}

fn styled_label(mode: AssMode) -> &'static str {
    match mode {
        AssMode::Keep => msg::subtitles_styled_keep(),
        AssMode::Mine => msg::subtitles_styled_mine(),
    }
}

/// The sentence under the page title: what the previewed preset is. A hand-edited look says so.
fn about(preset: LookPreset) -> &'static str {
    match preset {
        LookPreset::Blend => msg::subtitles_about_blend(),
        LookPreset::AppleTv => msg::subtitles_about_apple_tv(),
        LookPreset::Classic => msg::subtitles_about_classic(),
        LookPreset::Netflix => msg::subtitles_about_netflix(),
        LookPreset::Outline => msg::subtitles_about_outline(),
        LookPreset::LargePrint => msg::subtitles_about_large_print(),
        LookPreset::HighContrast => msg::subtitles_about_high_contrast(),
        LookPreset::Marquee => msg::subtitles_about_marquee(),
        LookPreset::Custom => msg::subtitles_about_custom(),
    }
}

/// The Settings root's value for the Subtitles row: the preset the look carries.
pub(super) fn summary(preset: LookPreset) -> &'static str {
    preset_label(preset)
}

// ---------------------------------------------------------------------------------------------
// the rows
// ---------------------------------------------------------------------------------------------

/// What the page shows besides the look: which sample line and which picture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Choices {
    sample: usize,
    scene: usize,
}

fn sample_name(sample: usize) -> &'static str {
    match sample {
        0 => msg::subtitles_sample_accents(),
        1 => msg::subtitles_sample_cjk(),
        2 => msg::subtitles_sample_rtl(),
        3 => msg::subtitles_sample_symbols(),
        4 => msg::subtitles_sample_italics(),
        _ => msg::subtitles_sample_styled(),
    }
}

fn scene_name(scene: usize) -> &'static str {
    match scene {
        0 => msg::subtitles_scene_day(),
        1 => msg::subtitles_scene_night(),
        _ => msg::subtitles_scene_snow(),
    }
}

/// The value a row reads out.
fn value_of(look: &SubtitleLook, choices: Choices, field: Field) -> Option<String> {
    Some(match field {
        Field::Preset => preset_label(look.preset).to_string(),
        Field::Sample => sample_name(choices.sample).to_string(),
        Field::Scene => scene_name(choices.scene).to_string(),
        Field::Font => font_label(look.font).to_string(),
        Field::Size if look.size_pct == 0 => msg::subtitles_value_auto().to_string(),
        Field::Size => percent(look.size_pct),
        Field::Color => text_color(look.color),
        Field::Opacity => percent(look.opacity),
        Field::Bold | Field::Blend => return None,
        Field::Edge => edge_label(look.edge).to_string(),
        Field::EdgeWidth => pixels(look.edge_w),
        Field::Back => back_label(look.back).to_string(),
        Field::BackOpacity => percent(look.back_opacity),
        Field::BackRadius if look.back_radius == 0 => msg::subtitles_value_square().to_string(),
        Field::BackRadius => pixels(look.back_radius),
        Field::BackPad => pixels(look.back_pad),
        Field::BackColor => box_color(look.back_color),
        Field::Anchor => anchor_label(look.anchor).to_string(),
        Field::Nudge => pixels(look.nudge),
        Field::LineSpacing => percent(look.line_pct),
        Field::Reveal => reveal_label(look.reveal).to_string(),
        Field::Styled => styled_label(look.ass).to_string(),
        Field::Reset => return None,
    })
}

fn label_of(field: Field) -> &'static str {
    match field {
        Field::Preset => msg::subtitles_row_preset(),
        Field::Sample => msg::subtitles_row_sample(),
        Field::Scene => msg::subtitles_row_scene(),
        Field::Font => msg::subtitles_row_font(),
        Field::Size => msg::subtitles_row_size(),
        Field::Color | Field::BackColor => msg::subtitles_row_color(),
        Field::Opacity | Field::BackOpacity => msg::subtitles_row_opacity(),
        Field::Bold => msg::subtitles_row_bold(),
        Field::Edge => msg::subtitles_row_edge(),
        Field::EdgeWidth => msg::subtitles_row_edge_width(),
        Field::Back => msg::subtitles_row_back(),
        Field::BackRadius => msg::subtitles_row_radius(),
        Field::BackPad => msg::subtitles_row_padding(),
        Field::Anchor => msg::subtitles_row_anchor(),
        Field::Nudge => msg::subtitles_row_nudge(),
        Field::LineSpacing => msg::subtitles_row_line_spacing(),
        Field::Blend => msg::subtitles_row_blend(),
        Field::Reveal => msg::subtitles_row_reveal(),
        Field::Styled => msg::subtitles_row_styled(),
        Field::Reset => msg::subtitles_row_reset(),
    }
}

/// The page's rows from plain inputs (the text-fit tests call this directly).
fn subtitles_form(look: &SubtitleLook, choices: Choices) -> Form<Field, Field, SettingsPage> {
    let row = |field: Field| {
        let mut row = Row::new(label_of(field));
        match field {
            Field::Bold => row = row.toggle(look.bold),
            Field::Blend => row = row.toggle(look.blend).detail(msg::subtitles_blend_detail()),
            Field::Reset => row = row.detail(msg::subtitles_reset_detail()).dim(adjust(*look, Field::Reset, 1, true) == *look),
            _ => {}
        }
        if let Some(value) = value_of(look, choices, field) {
            row = row.value(value);
        }
        row.dim(!applies(look, field))
    };
    let kind = |field: Field| match field {
        Field::Bold | Field::Blend => RowKind::Toggle,
        _ => RowKind::Button,
    };
    let section = |title: &'static str, fields: &[Field]| {
        fields.iter().fold(FormSection::new(title), |section, field| {
            section.item(*field, kind(*field), *field, row(*field)).disabled(!applies(look, *field))
        })
    };
    Form::new()
        .section(section(msg::subtitles_group_look(), &[Field::Preset, Field::Sample, Field::Scene]))
        .section(section(msg::subtitles_group_text(), &[Field::Font, Field::Size, Field::Color, Field::Opacity, Field::Bold]))
        .section(section(msg::subtitles_group_edge(), &[Field::Edge, Field::EdgeWidth]))
        .section(section(
            msg::subtitles_group_background(),
            &[Field::Back, Field::BackOpacity, Field::BackRadius, Field::BackPad, Field::BackColor],
        ))
        .section(section(msg::subtitles_group_position(), &[Field::Anchor, Field::Nudge, Field::LineSpacing]))
        .section(section(msg::subtitles_group_behavior(), &[Field::Blend, Field::Reveal, Field::Styled, Field::Reset]))
}

// ---------------------------------------------------------------------------------------------
// the preview
// ---------------------------------------------------------------------------------------------

/// The sample cues, as the SubRip file a server would hand the player: the text goes through the
/// player's own converter, so tags, bidirectional text and fallback faces behave as they do there.
const SAMPLES: [&str; 5] = [
    "1\n00:00:00,500 --> 00:00:06,500\nCafé au lait, naïve façade,\nZoë’s smörgåsbord costs €5½.\n",
    "1\n00:00:00,500 --> 00:00:06,500\n今日はいい天気ですね。\n오늘 날씨가 정말 좋네요 · 你好吗？\n",
    "1\n00:00:00,500 --> 00:00:06,500\nمرحبا بالعالم، كيف حالك؟\nשלום עולם, מה שלומך?\n",
    "1\n00:00:00,500 --> 00:00:06,500\n♪ Hit the road, Jack ♪\nIt’s 50° ★★★★☆ — 😀 🔥 © ½\n",
    "1\n00:00:00,500 --> 00:00:06,500\n<i>He whispers:</i> “Don’t go out there.”\n— <b>Bold</b> and <i>italic</i> and plain\n",
];

/// The sixth sample is a script with a look of its own (a bold italic serif, yellow, red edge), so the
/// Styled subtitles row shows what it does: kept as authored, or drawn in the viewer's look.
const AUTHORED: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\nWrapStyle: 1\n\
ScaledBorderAndShadow: yes\n\n[V4+ Styles]\n\
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, \
Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, \
MarginV, Encoding\n\
Style: Default,PlxSerif,64,&H0000E6FF,&H0000E6FF,&H000000C8,&H80000000,-1,-1,0,0,100,100,0,0,1,4,2,2,200,200,90,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,0:00:00.50,0:00:06.50,Default,,0,0,0,,A file that brings its own look:\\Nfont, colour and edge as its author set them.\n";

/// How many samples there are, the authored one last.
const SAMPLE_COUNT: usize = SAMPLES.len() + 1;

/// The loop of a sample cue, in milliseconds: the cue (0.5 to 6.5 s) and a pause, so Reveal as
/// spoken can be watched from its first word. With Reveal off the cue is held at its middle.
const LOOP_MS: u32 = 8000;
const HELD_MS: i64 = 3500;

/// A scene the sample is drawn over: the picture's top and bottom colour and its mean luma, which is
/// what the Blend tone follows.
struct Scene {
    top: [f32; 3],
    bottom: [f32; 3],
    luma: f32,
}

const SCENES: [Scene; 3] = [
    Scene { top: [0.46, 0.62, 0.80], bottom: [0.86, 0.78, 0.62], luma: 0.62 },
    Scene { top: [0.04, 0.05, 0.11], bottom: [0.08, 0.09, 0.16], luma: 0.07 },
    Scene { top: [0.90, 0.93, 0.97], bottom: [0.96, 0.97, 0.99], luma: 0.95 },
];

/// The picture is the whole 1920 by 1080 canvas scaled into the frame.
const CANVAS: Viewport = Viewport { x: 0, y: 0, width: 1920, height: 1080 };
/// The preview's width: the narrative column, which the preview rests on the floor of.
const PREVIEW_W: f32 = 880.0;
const PREVIEW_H: f32 = PREVIEW_W * 9.0 / 16.0;

fn preview_frame(layout: RouteLayout) -> Rect {
    let n = layout.narrative;
    Rect::new(n.x, n.y + n.h - PREVIEW_H, PREVIEW_W, PREVIEW_H)
}

fn page_layout() -> RouteLayout {
    RouteLayout::screen_with_copy_w(PREVIEW_W)
}

/// The sample cue as a source for `style`: one timeline per sample (`id`), restyled in place
/// (`revision`) when only the style moves, so the picture never blanks while libass catches up.
struct SampleSource {
    sample: usize,
    id: u64,
    revision: u64,
    generation: u32,
    source: Arc<ass::Source>,
}

impl SampleSource {
    fn build(sample: usize, id: u64, revision: u64, style: &TextStyle) -> Arc<ass::Source> {
        let (origin, script) = match SAMPLES.get(sample) {
            Some(srt) => (ass::Origin::Text(*style), text_ass::script_from_srt(srt).assemble(style)),
            None => (ass::Origin::Authored, AUTHORED.to_string()),
        };
        Arc::new(ass::Source {
            id,
            revision,
            origin,
            content: ass::Content::Script { bytes: Arc::from(script.into_bytes()), fonts: Arc::from([]) },
        })
    }

    /// The sample is a script with its own styling (it is not restyled by the viewer's picks).
    fn authored(&self) -> bool {
        self.sample >= SAMPLES.len()
    }

    fn of(sample: usize, style: &TextStyle) -> Self {
        let id = ass::next_source_id();
        Self { sample, id, revision: 1, generation: style.generation(), source: Self::build(sample, id, 1, style) }
    }
}

struct Preview {
    ass: AssSubtitles,
    source: Option<SampleSource>,
    /// The frame clock of the last tick, and of the one the loop counts from.
    now: u32,
    since: u32,
}

impl Preview {
    fn new() -> Self {
        Self { ass: AssSubtitles::default(), source: None, now: 0, since: 0 }
    }

    /// Ask libass for the frame the sample shows `now`: the page's own look, whole-screen frame, over
    /// the luma of the chosen scene.
    fn tick(&mut self, now: u32, look: &SubtitleLook, choices: Choices) {
        self.now = now;
        let style = TextStyle::of(plx_media::route::subtitle_size(), plx_media::route::subtitle_position(), look);
        match &mut self.source {
            Some(held) if held.sample == choices.sample => {
                if held.generation != style.generation() && !held.authored() {
                    held.revision += 1;
                    held.generation = style.generation();
                    held.source = SampleSource::build(held.sample, held.id, held.revision, &style);
                }
            }
            slot => *slot = Some(SampleSource::of(choices.sample, &style)),
        }
        let clock = if look.reveal == Reveal::Off { HELD_MS } else { i64::from(now.wrapping_sub(self.since) % LOOP_MS) };
        if let Some(held) = &self.source {
            // a script with its own styling is drawn in the viewer's look only when they asked for it
            let mine = (held.authored() && look.ass == AssMode::Mine).then_some(style);
            self.ass.preview(held.source.clone(), clock, CANVAS, SCENES[choices.scene].luma, now, mine);
        }
    }

    /// Does the preview move without a key: a reveal replaying, or the Blend tone easing?
    fn animating(&self, look: &SubtitleLook) -> bool {
        look.reveal != Reveal::Off || (look.blend && self.ass.easing())
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.ass.release();
    }
}

fn draw_scene(p: Painter, frame: Rect, scene: &Scene) {
    let rgb = |c: [f32; 3]| [c[0], c[1], c[2], 1.0];
    p.rect(frame, theme::CARD_RING_RAD, rgb(scene.top), rgb(scene.bottom), 0.0);
    // a horizon and two shapes standing on it, darker than the sky: enough picture for the eye to
    // judge a caption against something that is not flat
    let shade = |c: [f32; 3], k: f32| [c[0] * k, c[1] * k, c[2] * k, 1.0];
    let ground = Rect::new(frame.x, frame.y + frame.h * 0.74, frame.w, frame.h * 0.26);
    p.rrect(ground, 0.0, theme::CARD_RING_RAD, shade(scene.bottom, 0.62));
    for (x, w, h) in [(0.12, 0.16, 0.30), (0.62, 0.22, 0.42)] {
        let (w, h) = (frame.w * w, frame.h * h);
        let r = Rect::new(frame.x + frame.w * x, ground.y - h, w, h + 2.0);
        p.rrect(r, 6.0, 6.0, shade(scene.top, 0.55));
    }
}

// ---------------------------------------------------------------------------------------------
// the page
// ---------------------------------------------------------------------------------------------

pub(super) struct SubtitlesPage {
    entry: EntryId,
    form: FormTable<Field, Field, SettingsPage>,
    state: SubState,
    /// Writes sent and not yet answered; the rows read the stored look again when none is out.
    writes: Vec<std::sync::mpsc::Receiver<bool>>,
    preview: Preview,
    tag: std::ffi::CString,
}

struct SubState {
    look: SubtitleLook,
    choices: Choices,
    /// The focused row's key: an identity, not a position.
    sel: RowKey,
}

impl LogicalState for SubState {
    fn write(&self, w: &mut Canon) {
        w.u32(self.sel.0).u64(self.look.stamp()).u32(self.choices.sample as u32).u32(self.choices.scene as u32);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!(
            "subtitles sel={} preset={} sample={} scene={}",
            self.sel.0, self.look.preset.spelling(), self.choices.sample, self.choices.scene
        ));
    }
}

impl SubtitlesPage {
    pub(super) fn new(entry: EntryId) -> Self {
        let mut page = Self {
            entry,
            form: FormTable::new(super::super::registry::BAND),
            state: SubState { look: plx_media::route::subtitle_look(), choices: Choices { sample: 0, scene: 0 }, sel: Field::Preset.key() },
            writes: Vec::new(),
            preview: Preview::new(),
            tag: std::ffi::CString::default(),
        };
        page.rebuild();
        page
    }

    /// Re-derive the rows and the preview's tag from the shown look, keeping the cursor on its row by
    /// identity.
    fn rebuild(&mut self) {
        let keep = self.form.selected_id().copied().unwrap_or(Field::Preset);
        self.form.table.compact = false;
        self.form.set(subtitles_form(&self.state.look, self.state.choices), Some(&keep));
        self.form.table.list_focused = true;
        let c = self.state.choices;
        let tag = msg::subtitles_sample_tag(sample_name(c.sample), c.sample as i64 + 1, SAMPLE_COUNT as i64);
        self.tag = std::ffi::CString::new(tag).unwrap_or_default();
    }

    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(
            Header::new(page_layout(), Some(msg::settings_title()), msg::subtitles_title(), about(self.state.look.preset)),
            &self.form.table, GroupId(0), self.entry,
        ).keyed(&self.form).adjusting()
    }

    /// Send `next` through the one write door and show it meanwhile.
    fn send(&mut self, next: SubtitleLook, fx: &mut Effects<'_, InnerHost>) {
        if next == self.state.look {
            return;
        }
        let (reply, receipt) = std::sync::mpsc::channel();
        fx.push(Fx::App(AppFx::Preferences(super::super::registry::PreferenceCmd::Choose(
            plx_media::route::Choice::SubtitleLook(next), reply))));
        self.writes.push(receipt);
        self.state.look = next;
        self.rebuild();
        fx.invalidate(Provenance::Input);
    }

    /// Step the row with key `key` by `delta`; `wrap` is OK's cycling.
    fn adjust_row(&mut self, key: u32, delta: i32, wrap: bool, fx: &mut Effects<'_, InnerHost>) {
        let Some(field) = Field::ALL.iter().copied().find(|f| f.key().0 == key) else { return };
        // Reset is OK's alone: a stray LEFT or RIGHT must not wipe the look
        if !applies(&self.state.look, field) || (field == Field::Reset && !wrap) {
            return;
        }
        match field {
            Field::Sample => {
                self.state.choices.sample = (self.state.choices.sample as i32 + delta).rem_euclid(SAMPLE_COUNT as i32) as usize;
                self.preview.since = self.preview.now;
            }
            Field::Scene => {
                self.state.choices.scene = (self.state.choices.scene as i32 + delta).rem_euclid(SCENES.len() as i32) as usize;
            }
            _ => {
                let next = adjust(self.state.look, field, delta, wrap);
                if next.reveal != self.state.look.reveal {
                    self.preview.since = self.preview.now;
                }
                self.send(next, fx);
                return;
            }
        }
        self.rebuild();
        fx.invalidate(Provenance::Input);
    }

    /// With no write of this page's own out, the rows read the stored look: the player's Style pages
    /// edit it too, and a page that kept what it last wrote would send it back with the next pick.
    fn follow_stored(&mut self) -> bool {
        let stored = plx_media::route::subtitle_look();
        if stored == self.state.look {
            return false;
        }
        self.state.look = stored;
        self.rebuild();
        true
    }

    /// Settle answered writes. Optimistic picks are live at once and a refused save keeps the pick
    /// for the session, so an answer changes nothing the rows show.
    fn poll(&mut self) -> bool {
        self.writes.retain(|receipt| matches!(receipt.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
        self.writes.is_empty() && self.follow_stored()
    }
}

impl Machine<InnerHost> for SubtitlesPage {
    type Ev = ScreenEvent<InnerHost>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, InnerHost>, fx: &mut Effects<'_, InnerHost>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                // Seat the cursor explicitly, as the Language page does: the generic Enter runs
                // before queued mount effects.
                fx.push(Fx::Deliver(
                    MachineId::Instance(InstanceId(0)),
                    Delivery::Screen(ScreenEvent::Enter(Enter::Fresh {
                        focus: FocusTarget::Elem(FocusKey { entry: self.entry, elem: self.state.sel.0 }),
                    })),
                ));
                Handled::Yes
            }
            ScreenEvent::Unmount => {
                self.preview.ass.release();
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                let moved = self.poll();
                self.preview.tick(t.ms, &self.state.look, self.state.choices);
                if moved || self.preview.animating(&self.state.look) {
                    fx.invalidate(Provenance::Input);
                }
                self.form.table.update(t.dt(), page_layout().sectioned_table().h);
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
                if let Some(key) = form_activate(&self.form, *key, fx).map(|field| field.key().0) {
                    self.adjust_row(key, 1, true, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(plx_machine::machine::InputEvent {
                kind: plx_machine::machine::InputKind::Key { key: key @ (Key::Left | Key::Right), at_edge: true, .. }, ..
            }) => {
                if let Some(focused) = cx.focus.current {
                    self.adjust_row(focused.elem, if *key == Key::Left { -1 } else { 1 }, false, fx);
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

plx_ui::focusable_via_view!(SubtitlesPage, InnerHost, view);

impl Screen<InnerHost> for SubtitlesPage {
    fn name(&self) -> &'static str { "subtitles" }
    fn state(&self) -> &dyn LogicalState { &self.state }
    fn crumb(&self, _cx: &Cx<'_, InnerHost>) -> Option<Cow<'_, str>> { Some(Cow::Borrowed(msg::settings_title())) }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, InnerHost>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, InnerHost>) {
        plx_ui::screen::Part::<InnerHost>::draw(&mut self.view(), f, Rect::FULL);
        let frame = preview_frame(page_layout());
        draw_scene(f.painter, frame, &SCENES[self.state.choices.scene]);
        // The caption is drawn in screen space; a page still sliding in or out has no settled
        // place for it, so it waits for the page to arrive.
        if f.painter.opacity() >= 0.99 {
            let (on_screen, _, _) = f.painter.to_screen(frame);
            let map = crate::registry::Mapping { x: on_screen.x, y: on_screen.y, s: on_screen.w / CANVAS.width as f32 };
            self.preview.ass.draw_look(false, plx_media::player::text_style::HudBand::CLASSIC, theme::TEXT_PRIMARY, &self.state.look, map);
        }
        draw_tag(f.painter, frame, &self.tag, f.cx.measure);
    }
    fn render(&self) -> RenderStrategy { RenderStrategy::Page }
    fn focus_source(&self) -> FocusSource { FocusSource::Engine }
    fn hit_source(&self) -> HitSource { HitSource::Engine }
}

/// "Sample 1 of 5 · Accents" on a scrim in the picture's corner, so the picture says what it shows.
fn draw_tag(p: Painter, frame: Rect, tag: &std::ffi::CStr, measure: &dyn plx_machine::machine::Measure) {
    use plx_ui::label::{HAlign, Label};
    let w = measure.width(tag, theme::size::CAPTION, false) + 2.0 * theme::space::MD;
    let h = theme::size::CAPTION as f32 + theme::space::MD;
    let chip = Rect::new(frame.x + theme::space::MD, frame.y + theme::space::MD, w, h);
    p.rrect(chip, h * 0.5, h * 0.5, theme::scrim_black(0.55));
    Label::new(tag.as_ptr(), theme::size::CAPTION, theme::TEXT_PRIMARY).h(HAlign::Center).draw(p, chip);
}

/// The `(row key, field)` of every row the page lists: what the structural tests address.
#[cfg(test)]
pub(super) fn keys_for_test() -> Vec<(u32, &'static str)> {
    Field::ALL.iter().map(|f| (f.key().0, label_of(*f))).collect()
}

#[cfg(test)]
#[path = "settings_subtitles_tests.rs"]
mod tests;
