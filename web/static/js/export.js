// Export space: the exported frames, and saving them into the work folder.
import { $, setMsg, postJSON, postSSE } from './util.js';
import { setBusy } from './spaces.js';

let savedPath = null;      // what Show in Finder / Open folder opens
let canReveal = false;     // only when the server can open a folder here (Mac, Linux desktop)

// Back to "nothing exported yet" (a new roll, a new export).
export function resetExport() {
  $('export-empty').classList.remove('hidden');
  $('export-options').classList.add('hidden');
  $('frames-section').classList.add('hidden');
}

export async function loadFrames() {
  const { folder, frames, reveal } = await fetch('/api/frames').then(r => r.json());
  canReveal = !!reveal;
  if (reveal) $('save-reveal').textContent = reveal;
  if (!frames.length) return;
  $('export-folder').textContent = folder || 'the work folder';
  $('export-folder').title = folder || '';
  setMsg($('save-msg'), '');
  $('save-reveal').classList.add('hidden');
  $('save-prog-wrap').classList.remove('show');
  $('export-empty').classList.add('hidden');
  $('export-options').classList.remove('hidden');
  $('frames-section').classList.remove('hidden');
  $('frames-title').textContent = `Frames (${frames.length})`;
  $('export-title').textContent = `Save ${frames.length} frame${frames.length === 1 ? '' : 's'}`;
  const grid = $('frames-grid');
  grid.innerHTML = '';
  for (const { index } of frames) {
    const card = document.createElement('div');
    card.className = 'frame-card';
    card.innerHTML = `
      <img src="/api/frames/${index}/thumb?t=${Date.now()}"
           alt="Frame ${index}" loading="lazy">
      <div class="frame-footer">
        <span class="frame-num">Frame ${index}</span>
      </div>`;
    grid.appendChild(card);
  }
}

// ── Save to the work folder ──────────────────────────────────────────────
document.querySelectorAll('[data-save]').forEach(b => b.addEventListener('click', async () => {
  const all = document.querySelectorAll('[data-save]');
  all.forEach(x => x.disabled = true);
  setBusy('export', true);
  setMsg($('save-msg'), '');
  $('save-reveal').classList.add('hidden');
  $('save-prog-wrap').classList.add('show');
  $('save-prog').value = 0;
  $('save-prog-label').textContent = 'Starting…';
  await postSSE('/api/save', { fmt: b.dataset.save }, ev => {
    if (ev.type === 'progress') {
      $('save-prog').value = ev.pct;
      $('save-prog-label').textContent = ev.step;
    } else if (ev.type === 'done') {
      $('save-prog-wrap').classList.remove('show');   // the message says it
      savedPath = ev.file || ev.folder;
      setMsg($('save-msg'), ev.count === 1 && ev.file
        ? `Saved the ${ev.what}` : `Saved ${ev.count} ${ev.what}`, 'ok');
      $('save-msg').title = savedPath || '';
      $('save-reveal').classList.toggle('hidden', !canReveal);
    } else if (ev.type === 'error') {
      setMsg($('save-msg'), ev.message, 'err');
    }
  });
  all.forEach(x => x.disabled = false);
  setBusy('export', false);
}));
$('save-reveal').addEventListener('click', () => postJSON('/api/reveal', { path: savedPath }));
