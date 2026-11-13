// Frames space: place the crop boxes on the film strip (minilab style),
// then export them at full resolution.
import { $, setMsg, postJSON, postSSE, store } from './util.js';
import { currentSpace, onShow, showSpace, setBusy, fitBar } from './spaces.js';
import { loadFrames, resetExport } from './export.js';

let meta = null;      // prescan metadata
let crops = [];       // confirmed crops: {c: centre row, w: width} in full res
let cur = 0;          // current frame index
let zoom = 1;         // strip px per preview px

// The Frames empty/progress card: title (null keeps it), step, progress
// (null hides the bar).
export function framesWait(title, msg, pct) {
  $('frames-wait').classList.remove('hidden');
  if (title != null) $('frames-wait-title').textContent = title;
  $('frames-wait-msg').textContent = msg;
  $('frames-wait-prog-wrap').classList.toggle('show', pct != null);
  if (pct != null) $('frames-wait-prog').value = pct;
}

// Frames shows a message line only for an error ('' hides it).
export function framesError(text) {
  setMsg($('frames-msg'), text, 'err');
  $('frames-msg').hidden = !text;
}

// A processed roll (the prescan's `done` event) opens here.
export function openRoll(m) {
  meta = m;
  showIce();
  startConfirm();
}

// ── Export options: dust removal and sharpening ──────────────────────────
// Dust removal needs the scan's infrared channel: B&W scans without it
// (silver blocks IR), and older raws may lack the calibration's IR table.
// Slides get none: it softens slide grain (the prescan leaves meta.ice off).
// The choices are remembered in this browser.
function showIce() {
  const box = $('ice-on'), ok = !!(meta && meta.ice);
  box.disabled = !ok;
  $('ice-label').classList.toggle('off', !ok);
  $('ice-info').dataset.tip = ok
    ? 'Digital ICE: rebuilds dust and scratches from the infrared channel. Slower export.'
    : (meta && meta.film === 'bw'
       ? 'B&W is scanned without infrared, so there is nothing to find dust with.'
       : meta && meta.film === 'slide'
       ? 'Off for slides: it softens slide grain.'
       : 'This scan has no infrared channel to find dust with.');
}
$('sharpen').value = store.get('pakon-sharpen') || 'off';
$('sharpen').addEventListener('change', () => store.set('pakon-sharpen', $('sharpen').value));
$('ice-on').checked = store.get('pakon-ice') === '1';
$('ice-on').addEventListener('change', () => store.set('pakon-ice', $('ice-on').checked ? '1' : '0'));

// ── Confirm frames (interactive crop placement) ──────────────────────────
function startConfirm() {
  // Restore the boxes saved at this raw's last export, else none: the
  // operator auto-frames (Auto-frame, in the selected format) or places
  // them by hand, nudges them, adds with Enter and removes with Delete.
  // The full ribbon is shown.
  $('box-size').value = meta.format || 'full';
  const saved = meta.saved;
  crops = saved
    ? saved.centres.map((c, i) => ({ c, w: (saved.widths || [])[i] || meta.frame_w }))
    : [];
  cur = 0;
  $('strip-img').src = '/api/preview?t=' + Date.now();
  $('frames-wait').classList.add('hidden');
  $('confirm-card').classList.remove('hidden');
  fitStrip();
  $('strip-wrap').focus({ preventScroll: true });
}

// The strip is drawn `zoom` times the preview JPEG, so it fills most of the
// window height whatever the preview's own size.
// It starts at its largest and shrinks by whatever then runs past the
// window (the bar along the bottom has its room in the body's padding), so
// Frames fits without scrolling whatever sits below the strip.
function fitStrip() {
  if (!meta) return;
  fitBar();
  const size = h => {
    zoom = h / meta.preview_h;
    $('strip-img').style.height = Math.round(meta.preview_h * zoom) + 'px';
    $('strip-img').style.width = Math.round(meta.preview_w * zoom) + 'px';
    $('strip-inner').style.width = Math.round(meta.preview_w * zoom) + 'px';
  };
  const MIN = 160, MAX = 560;
  size(MAX);
  const over = document.documentElement.scrollHeight - window.innerHeight;
  if (over > 0) size(Math.max(MIN, MAX - over));
  renderStrip();
}
const confirmShown = () =>
  currentSpace === 'frames' && !$('confirm-card').classList.contains('hidden');
window.addEventListener('resize', () => { if (confirmShown()) fitStrip(); });
onShow(name => { if (name === 'frames' && meta) fitStrip(); });   // measured once it is on screen

const pxPerRow = () => meta.scale * zoom;              // full row -> strip px
const fullToPx = c => c * pxPerRow();
// Box width (full-res rows) for the size selected in the dropdown.
// The server sizes each format for the scan's resolution (meta.box_w; at
// Base 16 full 3000, half 1500, panoramic 5530).
const boxSize = () =>
  (meta.box_w && meta.box_w[$('box-size').value]) || meta.frame_w;

function renderStrip() {
  const inner = $('strip-inner');
  inner.querySelectorAll('.cropbox').forEach(e => e.remove());
  crops.forEach((cp, i) => {
    const w = cp.w * pxPerRow();
    const box = document.createElement('div');
    box.className = 'cropbox' + (i === cur ? ' active' : '');
    box.style.left = (fullToPx(cp.c) - w / 2) + 'px';
    box.style.width = w + 'px';
    box.innerHTML = `<span class="lbl">${i + 1}</span>`;
    box.onclick = () => { cur = i; renderStrip(); };
    inner.appendChild(box);
  });
  $('confirm-title').textContent = crops.length
    ? `Confirm frames — ${cur + 1} / ${crops.length}` : 'Place the frames';
  $('export-count').textContent = crops.length;
  $('export-btn').disabled = !crops.length;
  if (!crops.length) return;
  // keep the active box in view
  const wrap = $('strip-wrap'), x = fullToPx(crops[cur].c);
  if (x < wrap.scrollLeft + 80 || x > wrap.scrollLeft + wrap.clientWidth - 80)
    wrap.scrollLeft = x - wrap.clientWidth / 2;
}

function nudge(dpx) {                  // dpx in preview pixels -> full rows
  if (!crops.length) return;
  crops[cur].c = Math.max(0, Math.min(meta.rows,
    crops[cur].c + Math.round(dpx / pxPerRow())));
  renderStrip();
}
const step = () => Math.max(4, Math.round(meta.pitch * 0.004));  // micro (full rows)
function go(d) { cur = Math.max(0, Math.min(crops.length - 1, cur + d)); renderStrip(); }
function addFrame() {
  // Place the new box of the selected size immediately after the last one
  // (edge to edge), so full, half and panoramic frames tile correctly when added in a run.
  const w = boxSize();
  const last = crops[crops.length - 1];
  const c = last ? Math.min(meta.rows, last.c + Math.round((last.w + w) / 2))
                 : Math.round(w / 2);
  crops.push({ c, w }); cur = crops.length - 1; renderStrip();
}
function delFrame() {
  if (!crops.length) return;
  crops.splice(cur, 1); cur = Math.max(0, Math.min(cur, crops.length - 1)); renderStrip();
}

// Auto-frame: the OEM framing in the selected format replaces the boxes.
$('frame-auto').addEventListener('click', async () => {
  const btn = $('frame-auto'), fmt = $('box-size').value;
  btn.disabled = true;
  framesError('');
  try {
    const resp = await postJSON('/api/autoframe', { format: fmt });
    const d = await resp.json();
    if (!resp.ok) throw new Error(d.error || 'auto-frame failed');
    meta.format = d.format;
    meta.pitch = d.pitch;
    crops = d.centres.map(c => ({ c, w: d.frame_w }));
    cur = 0;
    renderStrip();
    if (!crops.length) framesError('No frames found in this format — place them by hand');
  } catch (err) {
    framesError(err.message);
  }
  btn.disabled = false;
  $('strip-wrap').focus({ preventScroll: true });
});
$('frame-add').addEventListener('click', addFrame);
$('frame-del').addEventListener('click', delFrame);
// Changing the dropdown resizes the currently selected box.
$('box-size').addEventListener('change', () => {
  if (crops.length) { crops[cur].w = boxSize(); renderStrip(); }
  $('strip-wrap').focus({ preventScroll: true });
});

// click on the strip background to start the current crop there (its
// leading edge, not its centre, lands on the click)
$('strip-inner').addEventListener('click', e => {
  if (e.target.closest('.cropbox')) return;
  const rect = $('strip-inner').getBoundingClientRect();
  if (!crops.length) { crops.push({ c: 0, w: boxSize() }); cur = 0; }
  crops[cur].c = Math.round((e.clientX - rect.left) / pxPerRow() + crops[cur].w / 2);
  renderStrip();
});

// keyboard: ←/→ nudge (Shift = coarse), ↑/↓ prev/next,
// Enter adds a box, Delete/Backspace removes the current one
window.addEventListener('keydown', e => {
  if (!confirmShown()) return;
  if (e.target.closest && e.target.closest('select, input, textarea')) return;  // a form control has the keys
  const k = e.key, big = e.shiftKey ? 6 : 1;
  if (k === 'ArrowLeft')       { nudge(-step() * big * pxPerRow()); }
  else if (k === 'ArrowRight') { nudge(step() * big * pxPerRow()); }
  else if (k === 'ArrowUp')    { go(-1); }
  else if (k === 'ArrowDown')  { go(1); }
  else if (k === 'Enter')      { addFrame(); }
  else if (k === 'Delete' || k === 'Backspace') { delFrame(); }
  else return;
  e.preventDefault();
});

// ── Export the confirmed crops at full resolution ────────────────────────
$('export-btn').addEventListener('click', async () => {
  const btn = $('export-btn');
  btn.disabled = true;
  setBusy('frames', true);
  $('exp-prog-wrap').classList.add('show');
  $('exp-prog').value = 0;
  resetExport();
  $('frames-grid').innerHTML = '';

  await postSSE('/api/export_frames',
    { centres: crops.map(c => Math.round(c.c)),
      widths: crops.map(c => Math.round(c.w)),
      ice: $('ice-on').checked && !$('ice-on').disabled,
      sharpen: $('sharpen').value },
    ev => {
      if (ev.type === 'progress') {
        $('exp-prog').value = ev.pct;
        $('exp-prog-label').textContent = ev.step;
      } else if (ev.type === 'done') {
        $('exp-prog').value = 1;
        $('exp-prog-label').textContent = 'Done';
        loadFrames().then(() => {
          $('exp-prog-wrap').classList.remove('show');   // the hints come back
          showSpace('export');
        });
      } else if (ev.type === 'error') {
        $('exp-prog-label').textContent = ev.message;
      }
    });
  btn.disabled = false;
  setBusy('frames', false);
});
