// Process space: the open raw, and processing it (the prescan: a long
// preview + detected frame centres), which opens the roll in Frames.
import { $, setMsg, postSSE } from './util.js';
import { showSpace, setBusy } from './spaces.js';
import { framesWait, framesError, openRoll } from './frames.js';
import { resetExport } from './export.js';

// The raw being processed: file name on top, its folder underneath.
// `dx`: the film the scanner read from the DX edge code (name, iso), if known.
export function showRaw(path, dx) {
  const cut = path ? path.lastIndexOf('/') : -1;
  $('raw-name').textContent = path ? path.slice(cut + 1) : 'No raw open';
  $('raw-dir').textContent = path ? path.slice(0, cut) : '';
  $('raw-slot').title = path || '';
  $('raw-slot').classList.toggle('set', !!path);
  $('raw-dx').textContent = dx
    ? `DX: ${dx.name ? dx.name + ' · ' : ''}ISO ${dx.iso}`
    : '';
  $('raw-dx').hidden = !dx;
}

export async function processRaw() {
  const btn = $('raw-open');
  btn.disabled = true;
  $('proc-film').disabled = true;
  setBusy('process', true);
  setBusy('frames', true);
  setMsg($('proc-msg'), '');
  $('proc-prog-wrap').classList.add('show');
  $('proc-prog').value = 0;
  framesWait('Processing ' + $('raw-name').textContent, 'Starting…', 0);
  $('confirm-card').classList.add('hidden');
  resetExport();
  $('frames-grid').innerHTML = '';

  await postSSE('/api/process', { film: $('proc-film').value }, ev => {
    if (ev.type === 'progress') {
      $('proc-prog').value = ev.pct;
      $('proc-prog-label').textContent = ev.step;
      framesWait(null, ev.step, ev.pct);
    } else if (ev.type === 'done') {
      $('proc-prog-wrap').classList.remove('show');   // the message says it
      setMsg($('proc-msg'), 'Ready in Frames', 'ok');
      openRoll(ev);
      showSpace('frames');
      framesError('');
    } else if (ev.type === 'error') {
      setMsg($('proc-msg'), ev.message, 'err');
      framesWait('Could not process the roll', ev.message, null);
    }
  });
  btn.disabled = false;
  $('proc-film').disabled = false;
  setBusy('process', false);
  setBusy('frames', false);
}

// Process follows the raw's film type (the scan's choice, or what the raw
// was last processed as); picking another type processes the open raw again.
$('proc-film').addEventListener('change', () => {
  if ($('raw-slot').classList.contains('set')) processRaw();
});
