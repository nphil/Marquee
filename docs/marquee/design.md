# Marquee design: audit of the current UI and the Lucent redesign

Marquee's UI moves to Lucent v2, TV profile, Rosé Pine palette (`/data/agent/managed-skills/design-language`), drawn by the 64-bit Rust UI. This file audits the PlxNative UI it replaces, states the redesign screen by screen, and records what the mocks prove. The mocks are `docs/marquee/mocks/*.html`; the rendered pictures are `docs/marquee/mocks/shots/*.jpg`, 1280 wide (every picture dark and light, nine of them also at 200 % text). Nothing here is deployed or built.

Scope: Home, Library, Detail, player chrome, track pickers, Settings, plus the shared kit (states, hint glyphs) and the playback-failure screen. Search, on-screen keyboard, profiles and PIN, sign-in, person page, watchlist, collections, the show page's season list, Up Next, trailers, the Info card and chapter strip are not drawn; they are built from the same kit.

## 1. Audit of the current UI against Lucent

Evidence is the committed demo screenshots (`docs/screenshots/*.jpg`), the source and the English strings. "Measured" means pixels of those screenshots: the background is the median of the text box, the ink the pixel farthest from it.

| # | Lucent rule | Current UI | Verdict |
|---|---|---|---|
| 1 | Hints are glyphs, not sentences | A standing capsule "Hold [OK] for options" on Home (`ui/src/hold_hint.rs`, string `widgets.hint.hold_options`); "Press [BACK] to return" on read-only panels (`screens/src/tracks_panel.rs`, `about_panel.rs`, `person_bio.rs`, `widgets.hint.return`); "Hold [BACK] to close PlxNative, then open it again" on Language (`settings.language.restart_hint`, `screens/src/settings.rs`). | Fail |
| 2 | No "OK" or "Back" drawn as words | `KeyHint` draws a ring around the letters `OK` and `BACK` (`ui/src/widgets.rs`, `hold_hint.rs:619`, `c"BACK"` in four screens). Sentences name the keys: `browse.json` "press OK for a new code", `playback_navigation.json` "With controls visible, OK confirms", `settings.json` Continue Watching help ("when you press OK… opened by holding OK"). The failure screen has a button labelled "Back" (`ux-failure.jpg`, `settings.back`). The Magic Remote has neither word printed on it. | Fail |
| 3 | Hover is a cue | `Hover::Focus` is the default stop (`ui/src/screen.rs`, `dispatch.rs`): moving the pointer over a tile moves focus to it ("hover parked focus on the new tile", `input_tests.rs`). | Fail |
| 4 | Click is the act (focus, then activate) | A click arms a press that commits on release inside the target (`press.rs`, `input_tests.rs`); the press machine is right. Because hover already moved focus, a click never has to focus first. | Keep the machine, change what hover does |
| 5 | Focus flows | Present and good: shelf glide at the scroll rate, a selection capsule that travels (`TabStrip`, `SelMark`), focus remembered per region, repeat pacing for held keys (`PANEL_REPEAT_MS`). Absent: one lit target that retargets from where it is; every tile pops its own 1.09 (cast 1.13, profiles 1.18). | Partial |
| 6 | Focus is light on the material, never a white pill; selection is not focus | The focused control is the brightest white face: the Resume pill, the item menu's first row, the sort menu's selected row (`ux-item-menu.jpg`, `ux-library-sort.jpg`; `theme::CONTROL_FOCUS_FACE_L` 0.972). `ACCENT` is `COOL_0`, the same white as text (`theme.rs`), so there is no accent hue, and the nav's selection capsule is also its focus mark. | Fail |
| 7 | Text is readable at 100 % and 200 % | Nine rungs 72 to 20 px (`gfx/src/gfx/tokens.rs`); `MICRO` 22 and `DIAGNOSTIC` 20 sit under Lucent's 24 px floor; the HUD title is a 54 px carve-out. No text-size setting exists and no check runs at 200 %; many layouts are pixel-placed constants (`FOOTER_H`, `PANEL_H`, `CTRL_Y`). The TV's own text engine draws the type, so there is no Manrope. | Fail |
| 8 | Contrast measured on the composited picture | Measured on seventeen text boxes in the demo screenshots: lowest 7.76:1 ("Recommended", library header), others 7.9 to 20.1 (detail meta 8.6, synopsis 8.9, player times 14.7 to 15.8, failure reason 8.0). The hero scrim is graded by an anchor-table test (`landing_hero.rs`, `widgets.rs`), the transport sits on a 0.86 bottom scrim. Not covered: bright video frames under the HUD, and any check at 200 %. | Pass on stills |
| 9 | One button size per screen | Round controls are 64 (`BTN_S`, `player_hud.rs`), matching Lucent's target. The Resume pill, the Info and Chapters pills and the library header's text controls are other sizes and shapes. | Partial |
| 10 | Chrome over media leaves, the way out never depends on it | HUD lingers 4.5 s (`LINGER_MS`), 8 s with a menu up (`HUD_MENU_MS`); a click on a hidden HUD reveals it and seeks nothing (`docs/testing-player-pointer.md`); Back works hidden. | Pass |
| 11 | Confirmation has an opaque backdrop | Dims run 0.30 to 0.72 over a latched, frosted copy of the page: menus 0.46 to 0.50, decisions 0.55, prose 0.72 (`theme.rs` `underlay::DIM_*`). Page words stay readable under every dim; Lucent needs 0.97 where the page must not read through. | Fail |
| 12 | Little copy | 1,069 English strings, 71 over 100 characters, 26 over 240 (`locales/en`). Legal and privacy text is long by nature; 14 non-legal Settings strings are over 100 characters (for example `settings.playback.force_note`). | Partial |
| 13 | Honest states | Failure shows verdict, reason and a technical footer, never red by decision (`ui/CLAUDE.md` rule 4); the detail meta line says "Direct Play". No view of how a file will play or what was tried. | Partial; the new playback ladder needs one |
| 14 | Dark and light, palette, material | Dark only, flat greys (measured grounds (38,43,46), (33,36,37), (51,42,40)), no light theme, no Rosé Pine, backdrop-blur glass only on the top bar and two capsules (`docs/DESIGN.md`). | Fail |
| 15 | Reduced motion honoured | No reduced-motion path in the Rust modules. [INFERENCE] webOS exposes no system flag; Lucent still wants an app switch. | Gap |

What stays, because it already matches Lucent or the Plex domain: the top navigation pill and the profile disc; the hero scrim idea and its graded anchors; captions that arrive under the focused card (`under_band`); 64 px round controls; the HUD fade; the track menu's grouping (Off, Forced only, "yours", Other languages, Timing, Style); per-region remembered focus; the repeat gate; the press machine.

## 2. Direction

**Identity.** Quiet light over artwork. The posters and backdrops are the only large colour; everything else is Rosé Pine canvas, tinted glass and ink. Accent lavender (`#A79BF5` dark, `#907AA9` light) has one job: the primary action, the selected mark, progress and an on switch. No white focus pill, no kicker lines, no cards in cards, no gradient text.

| Token | Value |
|---|---|
| Canvas | Palette canvas plus a soft wash (`--canvas-wash`); hero and detail put artwork under a scrim that fades into the canvas colour on the left and bottom |
| Glass over artwork or video | Near-opaque reading surface: dark `rgba(26,22,38,.82)`, light `rgba(255,250,243,.88)`; video chrome .90 and .94; overlays .97 |
| Radii | sheet 28, card 28, tile 22, row 22, control 16, pill 999 |
| Type | Manrope: display 92 (title when a film has no clear logo), title 40, body 28, label 26, caption 24; floor 24 at 100 %; 200 % doubles every size |
| Targets | Buttons and tabs 64, rows 80, one button size per screen; safe area x 96, y 54 |
| Motion | Focus, plates and scroll share one spring: 260 ms response, critically damped (`motion.spring`). Other Lucent tokens: press 90 ms, label 120, layer 220, exit 180; transform and opacity only; reduced motion makes focus instant and fades at most 120 ms. The mocks sample the spring with CSS `linear()`; the pictures are still frames |
| Tier | T1 (cached, no live blur except the small top pill if measured to pass); the tier is set by the frame probe on the TV, not by this document |

**Input model.**

- D-pad: one focus owner, an explicit adjacency graph per screen, no wrapping, no diagonal jumps, focus remembered per region, held keys paced (300 ms delay, 100 ms interval) and coalesced to the latest target.
- Flowing focus (LumaTV's rule): the outgoing and the incoming target move at the same time on one 260 ms critically damped spring (`motion.spring`). The old target settles back while the new one takes the lit state (brighter glass, edge, soft accent halo, 4 px lift, 1.02 to 1.06 scale); a key pressed mid-flight retargets from the current position and velocity, never restarts; the plate carrying the name arrives under a focused card on the same spring; and the scroll that reveals the target rides that spring too, moving only far enough to show the target plus 12 px. No second curve exists for focus, plate or scroll.
- Air pointer: hover shows the hover wash and a rim at 60 % of the raised edge on controls, and 30 % of the lit state (sheen and rim) on tiles; no lift, halo, scale or name plate, and it never moves focus. A click focuses the target and activates it on release inside. The cue lives from the last pointer movement to the next key press; that key hides the pointer and the D-pad focus shows again. Content scrolling under a still pointer does not bring the cue back.
- Focus cue: besides the lit state, every focused control draws a 2 px landing bar in ink (accent ink on the primary button); a focused card's plate carries it. The bar is what reaches 3:1 (section 5, decision 1).

**Hints are glyphs** (`mocks/00-kit.html?v=glyphs`). The set is the LumaTV one (`assets/glyphs`): OK is the wheel pressed (disc inside a ring), arrows are the ring with a chevron, Back is the curved return arrow, Hold is the key plus an hourglass (OK has its own ring-and-arc), Home the house, Voice the microphone, colour keys small bars. Size is 1.45 times the type beside it (37 px beside the 26 px label) and follows 200 % text; keys in ink, what they do in ink2. Each hint has one accessible name.

| Where | Glyphs | Accessible name | Replaces |
|---|---|---|---|
| Home, focused card, until used once | OK-hold, options | "Hold OK, options" | "Hold OK for options" capsule |
| Track menu, beside the tabs | Left, Right | "Left and Right, switch tab" | none (was implicit) |
| Language, restart pending | Back, hold, power | "Hold Back, close the app" | "Hold BACK to close PlxNative…" |
| Detail and Settings, top left | Back, then the place it returns to | "Back, Movies" | "Press BACK to return" and the crumb |

Everything else loses its sentence: the Continue Watching help drops "OK" and "holding OK"; the sign-in code line becomes a Refresh button; the failure screen's Back button becomes a close mark; transport help is the buttons themselves.

**Playback ladder.** Direct play, remux, software decode, server transcode, last resort. One chip names the route in four words with an icon: **Direct** (monitor-play), **Remux** (layers), **Decode** (cpu), **Convert** (server). It sits on the detail page beside resolution and CC, on the player title row, and, after a failure, as a row of crossed chips in the order tried. A subtitle that makes the server draw it says "Burned in" on its row.

## 3. Screens

Each paragraph: what the picture shows, the focus map, pointer behaviour, and what happens at 200 % text.

**Home** (`01-home.html`: `v=hero`, `v=shelves`, `v=menu`). Hero: the pinned title over its backdrop, text block bottom-left on the scrim (title or clear logo, one meta line, three lines of synopsis, then Resume as the one primary and two icon buttons, with "8 min left" beside them), page dots, and the first shelf peeking below. The hero does not rotate on a timer, only on input. Shelves: Continue Watching as 16:9 stills with a progress bar and a caption on every card (an episode needs its identity); other shelves as posters with no caption until focused. The focused card lifts, its caption becomes a plate with the landing bar and the hold-OK hint; the next card shows only the hover cue under the pointer. The item menu (Resume, Play from start, Mark as watched, Remove) opens beside the opener over a 0.97 scrim, with the opener still lit above it. Focus map: nav pill (Left/Right) is above hero actions (Left/Right), above the shelves; Down from hero actions goes to the last-focused shelf column; Up from the first shelf returns to the actions. A card cut by the screen edge draws no caption. At 200 %: hero synopsis clamps to two lines, dots and the peek go, shelf captions wrap, the second shelf is off screen.

**Library** (`02-library.html`: `v=grid`, `v=sort`). Title and count, one segmented tray (Library, Collections 6, Recommended; the selected tab has the accent mark), Sort as a button with a chevron and a filter icon button, then six posters across (268 × 402, the 12-column grid at gutter 24). Rows leave room for the plate. Posters carry a watched check or a progress bar; titles appear on the plate. Sort opens a popover over a 0.97 scrim with the current choice checked and the direction as its value. Focus map: header row above the grid, grid has no wrapping, Up from the top row returns to the last header control. At 200 %: title and controls stack on two lines, the grid keeps six columns, the plate grows beyond the poster's width.

**Detail** (`03-detail.html`). Backdrop with a scrim fading into the canvas, a Back-and-name crumb top left (also the pointer target for leaving), then logo, one meta line, three chips (resolution, CC, route), a clamped synopsis, and one primary (Resume with time left) beside four icon buttons (Play from start, Mark watched, Watchlist, More). Cast follows as circles with names; a person without a photo draws the user glyph, never an invented portrait. Credits, languages and Track information stay in the About block below, unchanged in behaviour. Focus map: crumb is reachable with Up from the actions, Down from the actions goes to the cast row. At 200 %: logo and meta stay, chips wrap, synopsis is two lines, cast moves below the fold.

**Player chrome** (`04-player.html`: `v=chrome`, `v=scrub`, `v=hidden`, `v=failed`). One reading surface along the bottom, one fade track for the whole group: out over 180 ms after 4.5 s without a key or pointer move, back over 120 ms; hidden, it takes no input and the first key only reveals it; Back works at all times. Row one: title, route chip, then Subtitles, Audio, Chapters, Info, More as 64 px icon buttons (the Info and Chapters word pills go). Row two: Play or Pause as the primary icon button, elapsed, the scrub well (chapter ticks, isolated thumb), remaining. The focused button's name floats above it on a plate, because an icon-only control must not rely on a tooltip. Scrubbing shows a preview card above the thumb and hides the subtitle. Subtitles lift above the chrome while it shows; their size is the viewer's own setting and ignores UI text size. `hidden` draws nothing over the picture. `failed` is a card on black: icon, "Can't play this file", one line, the four ladder chips crossed, Change quality (primary) and Try again, and a close mark; the Back key still leaves. At 200 %: button size stays 64 and labels grow, the title shrinks to 80 %, the plate and the subtitle do not collide.

**Track pickers** (`05-tracks.html`: `v=subs`, `v=audio`). A reading sheet anchored above the button cluster, over the picture only: the chrome stays at full contrast, the picture takes a 0.35 veil. A two-option tray (Audio, Subtitles) with the Left and Right glyphs, then rows of 72. Subtitles: Off, Forced only (value: language), tracks with a format value, a track the server draws with a server icon and "Burned in", Other languages (count, drill-in), a divider, Timing and Style as drill-ins. Audio: two-line rows (language, then codec and channels), "Default" as the value. The current choice has the accent check; focus is a separate lit row with the bar. Back closes the sheet, then the chrome stays. At 200 %: the sheet widens to 1000 and shows the first five rows; Timing and Style scroll in.

**Settings** (`06-settings.html`: `v=playback`, `v=language`). Back-and-name crumb, a 448 px rail of seven entries (tile, name, one live fact), a page pane. Video & playback: Quality (default quality, Direct play), then "If a file can't play as it is" with three switches (Remux, Decode on this TV, Convert on the server) in the order the ladder tries them, then Skip interval as a slider (Left and Right adjust, value on the right). A switch toggles on OK and never opens anything. Language is a radio list with a quiet "Restart to apply" state and the Hold-Back glyph hint. At 200 %: the rail widens, live facts drop, and the page shows the group in focus with the others scrolled out.

## 4. Mapping to the Rust UI

These are the places the redesign lands; the 32-bit code stays the behaviour reference.

- Palette and ink roles: `ui/src/theme.rs` (two layers kept, roles resolve to Rosé Pine dark and light; add `ink2-art` for words on scrimmed artwork, and resolve the light primary label to ink, decision 8).
- Size ladder: `gfx/src/gfx/tokens.rs`, five roles, a text-scale factor read by every `Label` and `TextView`; remove `MICRO` and the carve-outs.
- Focus and hover: `ui/src/screen.rs`, `dispatch.rs` (`Hover::Focus` becomes a cue; a click seats focus then activates), `widgets.rs` (one lit-state draw plus the landing bar), `card_row.rs` (plates, scale 1.04 to 1.06).
- Hints: `ui/src/widgets.rs` `KeyHint`, `ui/src/hold_hint.rs`, the four `c"BACK"` call sites; glyph atlas from LumaTV's `assets/glyphs`.
- Scrims: `theme.rs` `underlay::DIM_*`, `ui/src/decision_alert.rs`, `popover.rs`.
- Player: `appkit/src/player_hud.rs`, `track_menu.rs`; the ladder chip needs the route from the playback broker.
- Strings: `locales/*/widgets.json`, `settings.json`, `browse.json`, `playback_navigation.json`.
- Checks: port `mocks/render.mjs` rules to the renderer's recorded ink boxes so the same overlap and contrast gates run on real pictures.

## 5. Checks run on the mocks

`node docs/marquee/mocks/render.mjs` renders every picture with the system Chromium and checks it. Thresholds are Lucent's, doubled for 1920 × 1080:

- no text within 6 px of another text, within 12 px of the screen edge, clipped, crossing or within 6 px of a surface it is not on, or with under 16 px side padding on its own pill, plate or chip; no text under 24 px at 100 %;
- composited contrast: each text's ink, alpha included, over the worst pixel of what is really behind it (a second screenshot with every glyph transparent): 4.5:1, or 3:1 from 48 px up; words below 7:1 (the distance target) are counted, not failed;
- every landing bar against the surface directly below it: 3:1;
- any page error or failed request.

Text under a 0.97 scrim is hidden and not compared. Any mock opens with `?theme=light`, `?text=200`, `?v=<view>` and `?bar=off`.

Latest run **50 pictures** (16 views dark and light, 9 of them also at 200 % text), 718 texts, 60 landing bars. The final line of the run prints the exact lowest values; no check fails.

- Text: nothing overlaps, clips, crosses a surface or leaves the safe edge at 100 % or 200 %. At 200 % the layouts reflow instead of shrinking: Home's hero drops the dots and the shelf peek and clamps the synopsis to two lines, Library stacks its header and keeps six columns while the plate grows, Detail moves the cast below the fold, the track sheet and the Settings page show the rows around the focus and scroll the rest. Targets grow to 96 px and icons to 64 px with the type; they never shrink.
- Contrast: lowest 4.5:1 for text under 48 px and 4.3:1 for text from 48 px; 131 lines sit under the 7:1 distance target, almost all `ink2` detail lines in the light theme. Words on artwork use `ink` at .88 (`--ink-2-art`) instead of the palette's `ink2` (.66), which measured 4.1 to 4.4:1 over the light hero and detail.
- Focus: the landing bar measures 3.1:1 or better in every picture, and 10.1:1 (dark) and 18.3:1 (light) on a Settings row. Without the bar the lit state alone measures: face against page 1.5:1 dark and 1.04:1 light, 1px rim against page 3.05:1 dark and 2.04:1 light.
- Both themes: every view is drawn dark and light from the same markup and tokens; the light hero fades the picture into cream, and the light primary label is ink.

Not covered by these pictures: frame timing and blur cost at 120 Hz (the tier is set on the TV), bright video behind the chrome (the chrome is a .90 and .94 reading surface, so it does not depend on the frame), the TV's text engine versus Manrope rendering, German and French string lengths, and the Magic Remote itself.

## 6. Decisions for Nitin

1. **Landing bar or no underline.** Lucent says a 2 px landing bar in ink gives focus its 3:1 cue; LumaTV's mocks dropped it on 2026-10-23 ("no line or underline") and rely on lift, brighter glass and an accent halo. Measured on a Settings row: bar 10.1:1 and 18.3:1, rim alone 3.05:1 dark and 2.04:1 light. Recommendation: keep the bar on Marquee. `?bar=off` on any mock shows the other look.
2. **Hero rotation.** The mocks rotate the hero only on input. A timer is a loop at rest and adds OLED burn-in risk. Recommendation: no timer.
3. **Names of the playback ladder.** Direct, Remux, Decode, Convert (the chip and the failure row). Recommendation: use them.
4. **Light theme at launch.** Marquee has none today; Lucent wants both and every mock passes in both. Recommendation: ship both.
5. **Continue Watching as 16:9 stills with a caption on every card**, other shelves as posters with captions on focus. Recommendation: yes, an episode card needs its identity.
6. **Manrope** for all text, bundled as a Latin and Cyrillic subset (the app ships Belarusian), replacing the TV's own text engine. Recommendation: yes; the Cyrillic row renders in the mocks.
7. **A Text size setting** (100, 150, 200 %) in Settings. Lucent requires 200 % to work and nothing exists today. Recommendation: add it to Settings; subtitles keep their own size setting.
8. **Light-theme primary label is ink, not the palette's `accentInk` white.** White on `#907AA9` is 3.8:1, ink is 4.8:1. The change is at the adapter layer; the palette file stays. Recommendation: yes.
9. **Menus and confirmations hide the page** (a .97 scrim) with the opener still lit above it. A lighter veil lets neighbouring captions read across the menu edge. Today's dims are .30 to .72. Recommendation: yes for both.

## 7. Artwork credits

All pictures in the mocks are resized or cropped from openly licensed works; nothing else is changed. Sources are pinned in `tests/demo_library/assets.json`.

| Used as | Source | Licence | Author |
| --- | --- | --- | --- |
| Sintel, backdrop (hero, card) | [studio.blender.org/sintel](https://studio.blender.org/projects/sintel/) | CC BY 4.0 | © Blender Foundation / studio.blender.org |
| Tears of Steel, detail backdrop | [Tears_of_Steel_frame_07_3f.jpg](https://commons.wikimedia.org/wiki/File:Tears_of_Steel_frame_07_3f.jpg) | CC BY 3.0 | Blender Foundation / Project Mango |
| Tears of Steel, player picture | [Tears_of_Steel_frame_04_3e.jpg](https://commons.wikimedia.org/wiki/File:Tears_of_Steel_frame_04_3e.jpg) | CC BY 3.0 | Blender Foundation / Project Mango |
| Tears of Steel, card | [Tears_of_Steel_frame_01_2a.jpg](https://commons.wikimedia.org/wiki/File:Tears_of_Steel_frame_01_2a.jpg) | CC BY 3.0 | Blender Foundation / Project Mango |
| Spring, card | [studio.blender.org/spring](https://studio.blender.org/projects/spring/) | CC BY 4.0 | © Blender Foundation / studio.blender.org |
| Caminandes: Llamigos, card | [Blender_Foundation_-_Caminandes_-_Episode_3_-_Llamigos_-](https://commons.wikimedia.org/wiki/File:Blender_Foundation_-_Caminandes_-_Episode_3_-_Llamigos_-_Cover_thumbnail.png) | CC BY 4.0 | Blender Foundation, caminandes.com |
| Elephants Dream, card | [studio.blender.org/elephants-dream](https://studio.blender.org/projects/elephants-dream/) | CC BY 4.0 | © Blender Foundation / studio.blender.org |
| Tears of Steel, clear logo (the film's own title card) | [studio.blender.org/tears-of-steel](https://studio.blender.org/projects/tears-of-steel/) | CC BY 4.0 | © Blender Foundation / studio.blender.org |
| Sintel, poster | [Sintel_poster.jpg](https://commons.wikimedia.org/wiki/File:Sintel_poster.jpg) | CC BY 3.0 | (c) copyright Blender Foundation - durian.blender.org |
| Spring, poster | [Spring2019AlphaPosterBlender.jpg](https://commons.wikimedia.org/wiki/File:Spring2019AlphaPosterBlender.jpg) | CC BY 4.0 | Blender Foundation |
| Tears of Steel, poster | [Tos-poster.png](https://commons.wikimedia.org/wiki/File:Tos-poster.png) | CC BY 3.0 | Blender Foundation / Project Mango |
| Big Buck Bunny, poster | [Big_buck_bunny_poster_big.jpg](https://commons.wikimedia.org/wiki/File:Big_buck_bunny_poster_big.jpg) | CC BY 3.0 | (c) copyright Blender Foundation / www.bigbuckbunny.org |
| Charge, poster | [Charge-movie_poster.jpg](https://commons.wikimedia.org/wiki/File:Charge-movie_poster.jpg) | CC BY 4.0 | Blender Studio |
| Coffee Run, poster | [Coffee_Run-movie_poster.png](https://commons.wikimedia.org/wiki/File:Coffee_Run-movie_poster.png) | CC BY 4.0 | Hjalti Hjalmarsson (Blender Cloud) |
| Cosmos Laundromat, poster | [CosmosLaundromatPoster.jpg](https://commons.wikimedia.org/wiki/File:CosmosLaundromatPoster.jpg) | CC BY 4.0 | CC-BY 2015, Blender Foundation, cloud.blender.org |
| Elephants Dream, poster | [ElephantsDreamPoster.jpg](https://commons.wikimedia.org/wiki/File:ElephantsDreamPoster.jpg) | CC BY 4.0 | Bassam Kurdali, Andy Goralczyk, and the Blender Foundation |
| Sprite Fright, poster | [Sprite_Fright-movie_poster.jpg](https://commons.wikimedia.org/wiki/File:Sprite_Fright-movie_poster.jpg) | CC BY 4.0 | Blender Studio |
| A Trip to the Moon, poster | [Voyage_dans_la_Lune_affiche.jpg](https://commons.wikimedia.org/wiki/File:Voyage_dans_la_Lune_affiche.jpg) | Public domain | Georges Melies |
