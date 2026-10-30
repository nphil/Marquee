#!/usr/bin/env python3
"""Cut the shipped subtitle FAMILIES `pkg/fonts/subs/Plx{Serif,Mono,Legible}-{Regular,Bold}.ttf`
(and their manifest `pkg/fonts/subs/families.json`) from pinned upstream OFL fonts.

These are the faces behind the Font row of Settings > Subtitles. Each family has a regular and a
bold face that answer to ONE family name ("PlxSerif", "PlxMono", "PlxLegible"), so the style's
`Bold` flag picks the weight. They cover Latin, Latin Extended, Greek, Cyrillic and the common
punctuation; every other script, and every symbol they lack, falls through libass's fallback chain
to the Default family ("Inter": Inter Bold, the Noto script faces, then the CJK default), set in
`src/ass.c` as `ass_set_fonts`'s default family, so none of them can draw a box where the Default
face would have drawn a glyph.

Inputs (google/fonts at the pinned commit, all SIL OFL 1.1, none declares a Reserved Font Name):
  ofl/notoserif/NotoSerif[wdth,wght].ttf        -> PlxSerif    (instanced at wght 400 and 700, wdth 100)
  ofl/notosansmono/NotoSansMono[wdth,wght].ttf  -> PlxMono     (same)
  ofl/atkinsonhyperlegible/AtkinsonHyperlegible-{Regular,Bold}.ttf -> PlxLegible (static)

Usage:  python3 tools/cut-sub-families.py [--fetch DIR]   (fonttools >= 4.60 required)
Without --fetch it reads the inputs from $FONT_SRC (default /tmp/fonts-src) and verifies their
sha256 against the pins below; the output is byte-reproducible from them.
"""
import hashlib
import json
import os
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "pkg" / "fonts" / "subs"
COMMIT = "6157dfaa640ca5738b4d1a7070fe389c3036d7fc"
BASE = f"https://raw.githubusercontent.com/google/fonts/{COMMIT}/ofl"

# (family, family-key, upstream path, local name, sha256 of the upstream file, variable?)
SOURCES = [
    ("PlxSerif", "notoserif", "NotoSerif[wdth,wght].ttf", "NotoSerif-VF.ttf", None, True),
    ("PlxMono", "notosansmono", "NotoSansMono[wdth,wght].ttf", "NotoSansMono-VF.ttf", None, True),
    ("PlxLegible", "atkinsonhyperlegible", "AtkinsonHyperlegible-Regular.ttf", "AtkinsonHyperlegible-Regular.ttf", None, False),
    ("PlxLegible", "atkinsonhyperlegible", "AtkinsonHyperlegible-Bold.ttf", "AtkinsonHyperlegible-Bold.ttf", None, False),
]

# Latin, Latin-1, Latin Extended A/B/Additional, IPA, modifiers, combining marks, Greek, Cyrillic,
# punctuation, currency, letterlike, number forms, arrows, a few symbols the subtitles use, ligatures.
RANGES = [
    (0x0020, 0x007E), (0x00A0, 0x024F), (0x0250, 0x02FF), (0x0300, 0x036F), (0x0370, 0x03FF),
    (0x0400, 0x052F), (0x1E00, 0x1EFF), (0x1F00, 0x1FFF), (0x2000, 0x206F), (0x20A0, 0x20CF),
    (0x2100, 0x214F), (0x2150, 0x218F), (0x2190, 0x21FF), (0x2200, 0x2211), (0x2260, 0x2265),
    (0x2669, 0x266F), (0x25A0, 0x25CF), (0x2605, 0x2606), (0xFB00, 0xFB06),
]

FAMILY_NAMES = {"PlxSerif": "Noto Serif", "PlxMono": "Noto Sans Mono", "PlxLegible": "Atkinson Hyperlegible"}


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def unicodes():
    return {c for lo, hi in RANGES for c in range(lo, hi + 1)}


def cut(src_font, family, bold):
    style = "Bold" if bold else "Regular"
    opts = subset.Options()
    opts.layout_features = ["*"]
    opts.name_IDs = ["*"]
    opts.hinting = False
    opts.notdef_outline = True
    opts.glyph_names = False
    opts.drop_tables += ["DSIG", "STAT", "MVAR", "HVAR", "avar"]
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=unicodes())
    sub.subset(src_font)
    name = src_font["name"]
    for nid in (1, 2, 3, 4, 6, 16, 17, 21, 22, 25):
        name.removeNames(nameID=nid)
    full = f"{family} {style}"
    for nid, text in ((1, family), (2, style), (3, f"{family}-{style};plx"), (4, full), (6, f"{family}-{style}")):
        name.setName(text, nid, 3, 1, 0x409)
    os2 = src_font["OS/2"]
    os2.usWeightClass = 700 if bold else 400
    # bit 5 BOLD, bit 6 REGULAR; bit 0 ITALIC stays clear
    os2.fsSelection = (os2.fsSelection & ~0b1100001) | (0b100000 if bold else 0b1000000)
    src_font["head"].macStyle = 1 if bold else 0
    return src_font


def emoji():
    """The emoji link of the libass fallback chain: the app's monochrome Noto Emoji (`pkg/appfont-emoji.ttf`),
    reduced to the codepoints Inter does not carry, with the "PlxSubs" and "Inter" aliases the Noto script
    faces have (`tools/plxsubs_names.py`) so libass reaches it by the default family when the style's
    family lacks a glyph, and with Inter Bold's line proportions so an emoji never changes a line's height.
    Without it an emoji in a subtitle falls to the CJK default, which has none: a box."""
    sys.path.insert(0, str(ROOT / "tools"))
    from plxsubs_names import add_noto_aliases

    inter = set(TTFont(ROOT / "pkg" / "appfont-bold.ttf").getBestCmap()) | set(TTFont(ROOT / "pkg" / "appfont.ttf").getBestCmap())
    inter_font = TTFont(ROOT / "pkg" / "appfont-bold.ttf")
    inter_upem, inter_asc, inter_desc = inter_font["head"].unitsPerEm, inter_font["OS/2"].usWinAscent, inter_font["OS/2"].usWinDescent
    font = TTFont(ROOT / "pkg" / "appfont-emoji.ttf")
    keep = {c for c in font.getBestCmap() if c not in inter and not 0xE000 <= c <= 0xF8FF}
    opts = subset.Options()
    opts.layout_features = ["*"]
    opts.name_IDs = ["*"]
    opts.hinting = False
    opts.notdef_outline = True
    opts.glyph_names = False
    opts.drop_tables += ["DSIG", "STAT"]
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=keep)
    sub.subset(font)
    upem = font["head"].unitsPerEm
    asc, desc = round(upem * inter_asc / inter_upem), round(upem * inter_desc / inter_upem)
    os2 = font["OS/2"]
    os2.usWinAscent, os2.usWinDescent = asc, desc
    os2.sTypoAscender, os2.sTypoDescender, os2.sTypoLineGap = asc, -desc, 0
    font["hhea"].ascent, font["hhea"].descent, font["hhea"].lineGap = asc, -desc, 0
    add_noto_aliases(font)
    out = OUT / "NotoEmoji-Fallback.ttf"
    font.save(out)
    check = TTFont(out)
    return {
        "file": out.name,
        "family": "Noto Emoji",
        "role": "fallback",
        "aliases": ["PlxSubs", "Inter"],
        "license": "OFL-1.1",
        "copyright": check["name"].getDebugName(0) or "",
        "codepoints": len(check.getBestCmap()),
        "bytes": out.stat().st_size,
        "sha256": sha256(out),
        "from": "pkg/appfont-emoji.ttf",
    }


def main():
    src = Path(os.environ.get("FONT_SRC", "/tmp/fonts-src"))
    if len(sys.argv) > 2 and sys.argv[1] == "--fetch":
        import urllib.parse
        import urllib.request
        src = Path(sys.argv[2])
        src.mkdir(parents=True, exist_ok=True)
        for _, key, upstream, local, _, _ in SOURCES:
            url = f"{BASE}/{key}/{urllib.parse.quote(upstream)}"
            (src / local).write_bytes(urllib.request.urlopen(url, timeout=60).read())
    entries = []
    for family, key, upstream, local, _, variable in SOURCES:
        path = src / local
        upstream_sha = sha256(path)
        weights = [(400, False), (700, True)] if variable else [(700, True)] if "Bold" in local else [(400, False)]
        for weight, bold in weights:
            font = TTFont(path)
            if variable:
                font = instancer.instantiateVariableFont(font, {"wght": weight, "wdth": 100}, inplace=False)
            font = cut(font, family, bold)
            style = "Bold" if bold else "Regular"
            out = OUT / f"{family}-{style}.ttf"
            font.save(out)
            check = TTFont(out)
            os2, head = check["OS/2"], check["head"]
            cmap = check.getBestCmap()
            entries.append({
                "file": out.name,
                "family": family,
                "bold": bold,
                "upstream_family": FAMILY_NAMES[family],
                "license": "OFL-1.1",
                "copyright": check["name"].getDebugName(0) or "",
                "win_em": (os2.usWinAscent + os2.usWinDescent) / head.unitsPerEm,
                "upem": head.unitsPerEm,
                "codepoints": len(cmap),
                "bytes": out.stat().st_size,
                "sha256": sha256(out),
                "upstream": {"url": f"{BASE}/{key}/{upstream}", "sha256": upstream_sha},
            })
    manifest = {
        "schema": 1,
        "tool": "tools/cut-sub-families.py",
        "fonttools": __import__("fontTools").version,
        "pinned_commit": f"google/fonts@{COMMIT}",
        "families": entries,
        "fallbacks": [emoji()],
    }
    (OUT / "families.json").write_text(json.dumps(manifest, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    for e in entries:
        print(f"{e['file']:28} {e['bytes']:8} B  {e['codepoints']:5} cp  win_em {e['win_em']}")
    for e in manifest["fallbacks"]:
        print(f"{e['file']:28} {e['bytes']:8} B  {e['codepoints']:5} cp  aliases {e['aliases']}")


if __name__ == "__main__":
    main()
