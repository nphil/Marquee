/* Shared behaviour for the Marquee mocks.
 * Query: ?theme=dark|light  ?text=200 (200 % text)  ?v=<variant> (elements carry data-v="a b"; body[data-default] is the default). */
(function () {
  const q = new URLSearchParams(location.search);
  const root = document.documentElement;
  root.dataset.theme = q.get('theme') || 'dark';
  if (q.get('text')) root.dataset.text = q.get('text');
  if (q.get('bar')) root.dataset.bar = q.get('bar');
  const v = q.get('v') || document.body.dataset.default || '';
  root.dataset.v = v;
  document.querySelectorAll('[data-v]').forEach(e => {
    if (!e.dataset.v.split(/\s+/).includes(v)) e.hidden = true;
  });
  /* A scrim of .97 or more hides the page (Lucent section 3): what is under it is marked, and the checker does not compare it. */
  const hider = document.querySelector('.dim.hide:not([hidden])');
  if (hider && !hider.closest('[hidden]')) {
    [...document.body.children].forEach(c => { if (c.tagName !== 'SCRIPT' && !c.contains(hider)) c.dataset.behind = '1'; });
  }
  document.querySelectorAll('[data-t200]').forEach(e => {
    const t = e.dataset.t200;
    if (root.dataset.text === '200' && t === 'hide') e.hidden = true;
  });
  window.addEventListener('DOMContentLoaded', () => {
    if (window.Icons) Icons.fill(document);
    if (window.Glyphs) Glyphs.fill(document);
  });
  /* the Magic Remote pointer, drawn only where a mock shows hover */
  window.pointerSvg = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 3l14 7.5-6 1.8-2.4 6z" stroke-linejoin="round"/></svg>';
  window.addEventListener('DOMContentLoaded', () => {
    document.querySelectorAll('.pointer').forEach(p => { p.innerHTML = window.pointerSvg; });
  });
})();
