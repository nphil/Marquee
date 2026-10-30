//! The faces every subtitle renderer starts with, and the order it adds them (plan D6).
//!
//! libass here runs with no font provider: it finds a face by FAMILY NAME among the faces handed
//! to `plx_ass_add_font`, and anything those faces lack falls to the default path (the CJK file,
//! with no coverage check). So Thai, Devanagari, Arabic and the other scripts of the account
//! language picker are reachable only because a face named for them is in the library:
//!
//!   * `appfont.ttf`, `appfont-bold.ttf` - Inter. Bold also answers to the family **"PlxSubs"**
//!     (a name record under Windows langID 0x0407), which the synthesized subtitle style names.
//!   * `fonts/subs/NotoSans<Script>-Bold.ttf` - one per script, cut by `tools/cut-noto-subs.py`,
//!     listed in `fonts/subs/manifest.json`. Each answers to "PlxSubs" and "Inter" as well as to its
//!     own name, and carries no Latin, digit or punctuation codepoint Inter Bold has.
//!
//! **The order is load-bearing.** libass breaks a tie between equally good members of a family by
//! the order they were added, and takes the first member for code 0 without a coverage check, so
//! both Inter faces go in before every Noto face: Latin, digits and punctuation can never be drawn
//! by a Noto face. The Noto faces follow in manifest order.
//!
//!   * `fonts/subs/Plx{Serif,Mono,Legible}-{Regular,Bold}.ttf` - the Font row's families of
//!     Settings > Subtitles, cut by `tools/cut-sub-families.py` and listed in
//!     `fonts/subs/families.json`. Each family's two faces share ONE family name ("PlxSerif", ...) so
//!     the style's `Bold` flag picks the weight. They answer to no other name: a glyph one of them
//!     lacks is looked up in the default family ("Inter", set by `src/ass.c`), which has every
//!     script, so no family draws a box where Default would draw a glyph.
//!   * `fonts/subs/NotoEmoji-Fallback.ttf` - the emoji Inter does not carry, aliased to "PlxSubs" and
//!     "Inter" like a Noto script face, so an emoji reaches it from any family. It goes in LAST.
//!
//! **Read at create.** The files are read from disk each time a renderer is created, handed to
//! libass (`ass_add_font` copies the bytes) and dropped. Nothing here is a process-lifetime copy:
//! the resident cost is libass's one copy per live renderer.

use super::Fault;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Inter Regular, then Inter Bold.
const INTER: [&str; 2] = ["appfont.ttf", "appfont-bold.ttf"];
/// Where the Noto faces and their manifests sit, under the app directory.
const SUBS_DIR: &str = "fonts/subs";
const MANIFEST: &str = "manifest.json";
const FAMILIES: &str = "families.json";

#[derive(Deserialize)]
struct Manifest {
    faces: Vec<Entry>,
}

/// The family faces and the fallback faces: both optional, because a package without them still
/// shows every subtitle (the Font row's other families fall back to Default).
#[derive(Deserialize, Default)]
struct Families {
    #[serde(default)]
    families: Vec<Entry>,
    #[serde(default)]
    fallbacks: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    file: String,
}

/// A bare file name: the manifest is payload, but a path out of the directory is never right.
fn plain(file: &str) -> bool {
    !file.is_empty() && file.ends_with(".ttf") && !file.contains(['/', '\\', '\0']) && !file.starts_with('.')
}

/// Every bundled face, in the order the renderer adds it. A missing or unreadable manifest is
/// `FontsUnavailable`, like a missing Inter: a package without them is broken, and subtitles that
/// silently lose a script are worse than a subtitle error that says why.
pub(super) fn bundled(asset_dir: &Path) -> Result<Vec<PathBuf>, Fault> {
    let dir = asset_dir.join(SUBS_DIR);
    let manifest: Manifest = std::fs::read(dir.join(MANIFEST))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(Fault::FontsUnavailable)?;
    if manifest.faces.is_empty() {
        return Err(Fault::FontsUnavailable);
    }
    let mut files: Vec<PathBuf> = INTER.iter().map(|name| asset_dir.join(name)).collect();
    for entry in manifest.faces {
        if !plain(&entry.file) {
            return Err(Fault::FontsUnavailable);
        }
        files.push(dir.join(entry.file));
    }
    // The families, then the emoji fallback last. A hostile name is skipped, not an error: these
    // faces are an addition, and a manifest that names a path out of the directory adds nothing.
    let extra: Families = std::fs::read(dir.join(FAMILIES))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    for entry in extra.families.into_iter().chain(extra.fallbacks) {
        if plain(&entry.file) {
            files.push(dir.join(entry.file));
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests;
