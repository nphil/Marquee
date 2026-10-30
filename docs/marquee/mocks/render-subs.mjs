// Subtitle pages (07 to 09): same renderer and checks as render.mjs, with this page set, writing shots-subs/.
// Renders every mock at 1920×1080 with the system Chromium, writes shots/*.jpg (1280 wide, q80) and checks each picture the way Lucent asks
// (LANGUAGE.md section 11, thresholds doubled for 1920×1080):
//   * no text within 6 px of another text, within 12 px of the screen edge, or clipped by an overflow ancestor;
//   * no text crossing, or within 6 px of, the edge of a surface it is not on; 16 px side padding on its own plate;
//   * text at least 24 px (the couch floor) at 100 % text;
//   * composited contrast: each text's ink (computed colour, alpha included) over the WORST pixel of what is really behind
//     it (a second screenshot with every glyph made transparent): 4.5:1, 3:1 from 48 px up. Below 7:1 (Lucent's distance
//     target) is listed, not failed.
// Usage: node render.mjs [name-prefix ...]    PLAYWRIGHT_CORE=/path/to/playwright-core/index.mjs to override the import.
import { fileURLToPath, pathToFileURL } from 'url';
import path from 'path';
import fs from 'fs';

const core = process.env.PLAYWRIGHT_CORE || '/data/home/lucent-ha/node_modules/playwright-core/index.mjs';
const { chromium } = await import(pathToFileURL(core).href);
const dir = path.dirname(fileURLToPath(import.meta.url));
fs.mkdirSync(path.join(dir, 'shots-subs'), { recursive: true });

// [name, page, query]. Every picture is rendered dark and light; the 200 % text pictures are the reflow check.
const BASE = [
  ['subs-main', '07-subtitle-style.html', 'v=main'],
  ['subs-presets', '07-subtitle-style.html', 'v=presets'],
  ['subs-night', '07-subtitle-style.html', 'v=night'],
  ['subs-classic-cjk', '07-subtitle-style.html', 'v=cjk'],
  ['subs-outline-rtl', '07-subtitle-style.html', 'v=rtl'],
  ['subs-netflix-symbols', '07-subtitle-style.html', 'v=symbols'],
  ['subs-reveal', '07-subtitle-style.html', 'v=reveal'],
  ['fix-menu', '08-fix-subtitles.html', 'v=menu'],
  ['fix-external', '08-fix-subtitles.html', 'v=root-external'],
  ['fix-embedded', '08-fix-subtitles.html', 'v=root-embedded'],
  ['fix-setup', '08-fix-subtitles.html', 'v=root-setup'],
  ['fix-confirm', '08-fix-subtitles.html', 'v=confirm'],
  ['fix-searching', '08-fix-subtitles.html', 'v=searching'],
  ['fix-results', '08-fix-subtitles.html', 'v=results'],
  ['fix-noresults', '08-fix-subtitles.html', 'v=noresults'],
  ['fix-error', '08-fix-subtitles.html', 'v=error'],
  ['fix-downloading', '08-fix-subtitles.html', 'v=downloading'],
  ['fix-done', '08-fix-subtitles.html', 'v=done'],
  ['integ-disconnected', '09-integrations.html', 'v=disconnected'],
  ['integ-entry', '09-integrations.html', 'v=entry'],
  ['integ-testing', '09-integrations.html', 'v=testing'],
  ['integ-connected', '09-integrations.html', 'v=connected'],
  ['integ-unreachable', '09-integrations.html', 'v=unreachable'],
  ['integ-badkey', '09-integrations.html', 'v=badkey'],
];
const TEXT200 = ['subs-main', 'fix-external'];
const SHOTS = [];
for (const [n, f, q] of BASE) {
  SHOTS.push([n, f, q, 'dark', 100]);
  SHOTS.push([n + '-light', f, q, 'light', 100]);
  if (TEXT200.includes(n)) { SHOTS.push([n + '-text200', f, q, 'dark', 200]); SHOTS.push([n + '-light-text200', f, q, 'light', 200]); }
}
const only = process.argv.slice(2);
const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', args: ['--no-sandbox', '--force-device-scale-factor=1', '--allow-file-access-from-files'] });
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1 });
const problems = [], notes = [];
const totals = { pictures: 0, texts: 0, bars: 0, lowestText: 99, lowestLarge: 99, lowestBar: 99, under7: 0 };
page.on('pageerror', e => problems.push('PAGE ERROR: ' + e.message));
page.on('requestfailed', q => problems.push('REQUEST FAILED: ' + q.url()));
const SURF = '.glass,.overlay,.plate,.btn,.chip,.nav,.seg>span,.badge,.opt,.row,.toast,.cap';

for (const [name, file, q, theme, text] of SHOTS) {
  if (only.length && !only.some(o => name.startsWith(o))) continue;
  const query = [q, 'theme=' + theme, text === 200 ? 'text=200' : ''].filter(Boolean).join('&');
  await page.goto('file://' + path.join(dir, file) + '?' + query);
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(150);
  const full = (await page.screenshot({ type: 'png' })).toString('base64');
  const jpg = await page.evaluate(async (full) => {
    const img = await createImageBitmap(await (await fetch('data:image/png;base64,' + full)).blob());
    const cv = new OffscreenCanvas(1280, 720); const cx = cv.getContext('2d'); cx.imageSmoothingQuality = 'high'; cx.drawImage(img, 0, 0, 1280, 720);
    const b = await cv.convertToBlob({ type: 'image/jpeg', quality: 0.8 });
    const u = new Uint8Array(await b.arrayBuffer()); let s = ''; for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode.apply(null, u.subarray(i, i + 0x8000)); return btoa(s);
  }, full);
  fs.writeFileSync(path.join(dir, 'shots-subs', name + '.jpg'), Buffer.from(jpg, 'base64'));

  const boxes = await page.evaluate((SURF) => {
    const out = [];
    const surfs = [...document.querySelectorAll(SURF)].filter(e => (e.offsetParent !== null || getComputedStyle(e).position === 'fixed') && !e.closest('[hidden]') && !(e.closest('[data-behind]') && !e.closest('.above')));
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    let n;
    while ((n = walker.nextNode())) {
      const t = n.textContent.replace(/\s+/g, ' ').trim();
      if (!t) continue;
      const el = n.parentElement;
      if (!el || el.closest('[hidden],script,style,title')) continue;
      if (el.closest('[data-behind]') && !el.closest('.above')) continue; // under a scrim of .97 or more: hidden, not compared
      const cs = getComputedStyle(el);
      if (cs.visibility === 'hidden' || cs.display === 'none') continue;
      const range = document.createRange(); range.selectNodeContents(n);
      const rects = [...range.getClientRects()].filter(r => r.width > 0.5 && r.height > 0.5);
      if (!rects.length) continue;
      let r = range.getBoundingClientRect();
      const lc = getComputedStyle(el).webkitLineClamp;
      if (lc && lc !== 'none') { const b = el.getBoundingClientRect(); r = new DOMRect(b.left, b.top, b.width, b.height); } // a clamped paragraph ends in an ellipsis: its box is the element's
      // the glyph box, not the line box: line-height padding is not ink
      const fs = parseFloat(cs.fontSize);
      const gy = r.top + (r.height - fs) / 2 + fs * 0.12, gh = fs * 0.8;
      let op = 1; for (let e = el; e; e = e.parentElement) op *= parseFloat(getComputedStyle(e).opacity);
      let clip = null;
      for (let e = el; e && e !== document.body; e = e.parentElement) {
        const c = getComputedStyle(e);
        if (c.overflow !== 'visible' || c.overflowX !== 'visible' || c.overflowY !== 'visible') {
          const b = e.getBoundingClientRect();
          if (r.left < b.left - 1 || r.right > b.right + 1 || r.top < b.top - 1 || r.bottom > b.bottom + 1) { clip = e.className || e.tagName; break; }
        }
      }
      const own = el.closest(SURF);
      const crossed = [];
      surfs.forEach(s => {
        if (s.contains(el)) return;
        const b = s.getBoundingClientRect();
        const ix = Math.min(r.right + 6, b.right) - Math.max(r.left - 6, b.left), iy = Math.min(r.bottom + 6, b.bottom) - Math.max(r.top - 6, b.top);
        if (ix > 0 && iy > 0) crossed.push((s.className || s.tagName).toString().slice(0, 40));
      });
      let padX = null;
      if (own) { const b = own.getBoundingClientRect(); padX = Math.min(r.left - b.left, b.right - r.right); }
      out.push({ t: t.slice(0, 48), x: r.left, y: gy, w: r.width, h: gh, fs, color: cs.color, op, clip, crossed, padX, own: own ? (own.className || own.tagName).toString().slice(0, 30) : '', el: el.tagName + '.' + (el.className || '').toString().slice(0, 30), id: n.__id });
    }
    return out;
  }, SURF);

  // second picture: every glyph transparent, so the real background behind each text can be sampled
  await page.addStyleTag({ content: '*{color:transparent!important;-webkit-text-fill-color:transparent!important;text-shadow:none!important}' });
  const bg = (await page.screenshot({ type: 'png' })).toString('base64');
  const contrast = await page.evaluate(async ({ bg, boxes }) => {
    const img = await createImageBitmap(await (await fetch('data:image/png;base64,' + bg)).blob());
    const cv = new OffscreenCanvas(img.width, img.height); const cx = cv.getContext('2d'); cx.drawImage(img, 0, 0);
    const lin = c => { c /= 255; return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); };
    const lum = c => 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
    const parse = s => { const m = s.match(/rgba?\(([^)]+)\)/) || s.match(/color\(srgb ([^)]+)\)/); const p = m[1].split(/[ ,\/]+/).filter(Boolean).map(Number); const srgb = s.startsWith('color('); return [p.slice(0, 3).map(v => srgb ? v * 255 : v), p.length > 3 ? p[3] : 1]; };
    return boxes.map(b => {
      const x = Math.max(0, Math.floor(b.x)), y = Math.max(0, Math.floor(b.y)), w = Math.max(1, Math.min(1920 - x, Math.ceil(b.w))), h = Math.max(1, Math.min(1080 - y, Math.ceil(b.h)));
      const d = cx.getImageData(x, y, w, h).data;
      const px = []; for (let i = 0; i < d.length; i += 4) px.push([d[i], d[i + 1], d[i + 2]]);
      px.sort((a, c) => lum(a) - lum(c));
      const [ink, a] = parse(b.color); const alpha = a * b.op;
      const ratioOn = bgp => { const comp = ink.map((v, i) => v * alpha + bgp[i] * (1 - alpha)); const A = lum(comp), B = lum(bgp); return (Math.max(A, B) + .05) / (Math.min(A, B) + .05); };
      // worst sample: the bg pixel (3rd or 97th percentile) closest to the ink's luminance
      const lo = px[Math.floor(px.length * 0.03)], hi = px[Math.floor(px.length * 0.97)];
      return Math.min(ratioOn(lo), ratioOn(hi));
    });
  }, { bg, boxes });

  // landing bar: the 2 px mark under a focused control, measured against what is above it (3:1 is the cue target)
  const bars = await page.evaluate(() => [...document.querySelectorAll('.btn.f,.nav.f,.row.f,.opt.f,.seg>span.f,.plate')].filter(e => !e.closest('[hidden]') && getComputedStyle(e).display !== 'none').map(e => { const r = e.getBoundingClientRect(); return { n: (e.className || '').toString().slice(0, 24), x: r.left, y: r.top, w: r.width, h: r.height }; }));
  const cue = await page.evaluate(async ({ bg, bars }) => {
    const img = await createImageBitmap(await (await fetch('data:image/png;base64,' + bg)).blob());
    const cv = new OffscreenCanvas(img.width, img.height); const cx = cv.getContext('2d'); cx.drawImage(img, 0, 0);
    const lin = c => { c /= 255; return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); };
    const lum = c => 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
    return bars.map(b => {
      const x0 = Math.round(b.x + b.w * 0.4), w = Math.max(4, Math.round(b.w * 0.2));
      const strip = (y, h) => { const d = cx.getImageData(x0, Math.round(y), w, h).data; const out = []; for (let i = 0; i < d.length; i += 4) out.push([d[i], d[i + 1], d[i + 2]]); return out; };
      let best = 1;
      const ref = strip(b.y + b.h - 2, 1); const rm = ref.reduce((a, p) => a + lum(p), 0) / ref.length;
      for (let dy = 4; dy <= 10; dy++) { const s = strip(b.y + b.h - dy, 1); const m = s.reduce((a, p) => a + lum(p), 0) / s.length; best = Math.max(best, (Math.max(m, rm) + .05) / (Math.min(m, rm) + .05)); }
      return { n: b.n, ratio: best };
    });
  }, { bg, bars });
  const fails = [], info = [];
  totals.pictures++; totals.texts += boxes.length; totals.bars += cue.length;
  cue.forEach(c => { totals.lowestBar = Math.min(totals.lowestBar, c.ratio); });
  boxes.forEach((b, i) => { if (b.fs >= 48) totals.lowestLarge = Math.min(totals.lowestLarge, contrast[i]); else totals.lowestText = Math.min(totals.lowestText, contrast[i]); if (contrast[i] < 7 && b.fs < 48) totals.under7++; });
  cue.forEach(c => { if (c.ratio < 3) fails.push(`focus cue ${c.ratio.toFixed(2)} < 3 (${c.n})`); });
  boxes.forEach((b, i) => {
    const tag = `"${b.t}"`;
    b.ratio = contrast[i];
    if (b.x < 12 || b.y < 12 || b.x + b.w > 1908 || b.y + b.h > 1068) fails.push(`edge ${tag} (${Math.round(b.x)},${Math.round(b.y)})`);
    if (b.clip) fails.push(`clipped ${tag} by ${b.clip}`);
    if (b.crossed.length) fails.push(`crosses/near surface ${tag} ~ ${b.crossed[0]}`);
    if (b.padX !== null && b.padX < 16 && !/glass|overlay|row/.test(b.own)) fails.push(`pad ${Math.round(b.padX)}px ${tag} on ${b.own}`);
    if (text === 100 && b.fs < 24) fails.push(`small ${b.fs}px ${tag}`);
    const need = b.fs >= 48 ? 3 : 4.5;
    if (b.ratio < need) fails.push(`contrast ${b.ratio.toFixed(2)} < ${need} ${tag}`);
    else if (b.ratio < 7 && b.fs < 48) info.push(`${b.ratio.toFixed(1)} ${tag}`);
  });
  for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
    const a = boxes[i], c = boxes[j];
    if (a.x - 6 < c.x + c.w && c.x - 6 < a.x + a.w && a.y - 6 < c.y + c.h && c.y - 6 < a.y + a.h) fails.push(`text gap ${JSON.stringify(a.t)} ~ ${JSON.stringify(c.t)}`);
  }
  const low = boxes.length ? Math.min(...boxes.map(b => b.ratio)) : 0;
  console.log(`${fails.length ? 'FAIL' : 'ok  '} ${name}  texts ${boxes.length}  lowest contrast ${low.toFixed(1)}${info.length ? `  (${info.length} under 7:1)` : ''}`);
  if (fails.length) problems.push(name + ':\n   ' + [...new Set(fails)].slice(0, 12).join('\n   '));
  if (info.length) notes.push(name + ': ' + info.slice(0, 6).join(' | '));
}
await browser.close();
console.log(`\n${totals.pictures} pictures, ${totals.texts} texts (lowest under 48 px ${totals.lowestText.toFixed(1)}:1, lowest from 48 px ${totals.lowestLarge.toFixed(1)}:1, ${totals.under7} under 7:1), ${totals.bars} landing bars (lowest ${totals.lowestBar.toFixed(1)}:1)`);
if (notes.length && process.env.VERBOSE) console.log('\nUNDER 7:1 (distance target, listed only):\n' + notes.join('\n'));
if (problems.length) { console.log('\nCHECKS FAILED:\n' + problems.join('\n')); process.exitCode = 1; } else console.log('\nall checks passed');
