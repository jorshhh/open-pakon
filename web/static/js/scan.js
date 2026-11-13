// Scan space: firmware load, scan, stop, advance film.
import { $, setMsg, postSSE, logLine, store } from './util.js';
import { showSpace, setBusy } from './spaces.js';
import { showRaw, processRaw } from './process.js';

// ── Firmware load ────────────────────────────────────────────────────────
$('fw-btn').addEventListener('click', async () => {
  const btn = $('fw-btn');
  btn.disabled = true;
  setMsg($('fw-msg'), '');
  $('fw-prog-wrap').classList.add('show');
  $('fw-prog').removeAttribute('value');   // indeterminate
  $('fw-log').textContent = '';
  $('fw-log').classList.add('show');

  const lines = [];
  await postSSE('/api/firmware', {}, ev => {
    if (ev.type === 'log') {
      lines.push(ev.message);
      const el = $('fw-log');
      el.textContent = lines.join('\n');
      el.scrollTop = el.scrollHeight;
    } else if (ev.type === 'done') {
      $('fw-prog').value = 1; $('fw-prog').max = 1;
      setMsg($('fw-msg'), 'Firmware loaded — scanner re-enumerating', 'ok');
    } else if (ev.type === 'error') {
      setMsg($('fw-msg'), ev.message, 'err');
    }
  });

  btn.disabled = false;
});

// ── Scan ─────────────────────────────────────────────────────────────────
$('scan-btn').addEventListener('click', async () => {
  const btn = $('scan-btn');
  btn.disabled = true;
  $('eject-btn').disabled = true;
  // While the scan runs, Stop takes Advance film's place.
  $('eject-btn').classList.add('hidden');
  $('stop-btn').classList.remove('hidden');
  $('stop-btn').disabled = false;
  $('stop-btn').textContent = 'Stop';
  setBusy('scan', true);
  setMsg($('scan-msg'), 'Starting…');
  $('scan-prog-wrap').classList.add('show');
  // A roll's raw shrinks with the pixel count: 56% at Base 8, 25% at Base 4,
  // and by a quarter for silver B&W, which scans without the IR channel.
  const base = Number($('scan-base').value);
  $('scan-prog').max = Math.round(2200 * { 16: 1, 8: 0.5625, 4: 0.25 }[base] *
                                  ($('scan-film').value === 'bw' ? 0.75 : 1));
  $('scan-prog').value = 0;
  $('scan-log').textContent = '';
  $('feed-prompt').classList.remove('show');
  await postSSE('/api/scan', { film: $('scan-film').value, base }, ev => {
    if (ev.type === 'order') {
      logLine('scan-log', `Order ${ev.order}: ${ev.path}`);
    } else if (ev.type === 'log') {
      logLine('scan-log', ev.message);
    } else if (ev.type === 'feed') {
      $('feed-prompt').textContent = ev.message;
      $('feed-prompt').classList.add('show');
      logLine('scan-log', ev.message);
    } else if (ev.type === 'progress') {
      $('scan-prog').value = ev.mb;
      $('scan-prog-label').textContent = `${ev.mb} MB received`;
      $('scan-msg').textContent = '';
    } else if (ev.type === 'done') {
      $('scan-prog').value = $('scan-prog').max;
      $('scan-prog-label').textContent = `Complete — ${ev.mb} MB`;
      setMsg($('scan-msg'), `Scan complete (${ev.mb} MB)`, 'ok');
      if (ev.path) {
        showRaw(ev.path, ev.dx);
        if (ev.film) $('proc-film').value = ev.film;
        showSpace('frames');
        processRaw();
      }
    } else if (ev.type === 'error') {
      setMsg($('scan-msg'), ev.message, 'err');
    }
  });

  $('feed-prompt').classList.remove('show');
  $('stop-btn').classList.add('hidden');
  $('eject-btn').classList.remove('hidden');
  btn.disabled = false;
  $('eject-btn').disabled = false;
  setBusy('scan', false);
});

// ── Stop: end the roll now (for a strip the end-of-roll check misses) ──
$('stop-btn').addEventListener('click', async () => {
  $('stop-btn').disabled = true;
  $('stop-btn').textContent = 'Stopping…';
  logLine('scan-log', 'Stop pressed — ending the roll and stopping the motor');
  try {
    const r = await fetch('/api/scan/stop', { method: 'POST' });
    if (!r.ok) {
      const d = await r.json().catch(() => ({}));
      logLine('scan-log', `Stop: ${d.error || r.status}`);
    }
  } catch (e) {
    logLine('scan-log', `Stop failed: ${e}`);
  }
});

// ── Advance film (runs the film out of the transport) ─────────────────
$('eject-btn').addEventListener('click', async () => {
  const btn = $('eject-btn');
  btn.disabled = true;
  $('scan-btn').disabled = true;
  setMsg($('scan-msg'), 'Advancing film…');
  $('scan-log').textContent = '';
  await postSSE('/api/eject', {}, ev => {
    if (ev.type === 'log') {
      logLine('scan-log', ev.message);
      if (ev.message.startsWith('eject:'))
        setMsg($('scan-msg'), ev.message.slice(7), 'ok');
    } else if (ev.type === 'done') {
      if ($('scan-msg').textContent === 'Advancing film…')
        setMsg($('scan-msg'), 'Transport stopped', 'ok');
    } else if (ev.type === 'error') {
      setMsg($('scan-msg'), ev.message, 'err');
    }
  });
  btn.disabled = false;
  $('scan-btn').disabled = false;
});

// The scan's film type and resolution are remembered in this browser.
$('scan-film').value = store.get('pakon-scan-film') || 'c41';
$('scan-film').addEventListener('change', () => store.set('pakon-scan-film', $('scan-film').value));
$('scan-base').value = store.get('pakon-scan-base') || '16';
if (!$('scan-base').value) $('scan-base').value = '16';
$('scan-base').addEventListener('change', () => store.set('pakon-scan-base', $('scan-base').value));
