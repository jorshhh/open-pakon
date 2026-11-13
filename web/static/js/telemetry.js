// Crash reports / usage statistics (pakon-web telemetry.rs): the consent
// popup, the Settings switches, and the reports themselves.
import { $, setMsg, postJSON } from './util.js';
import { currentSpace, onShow } from './spaces.js';

// Page switches and page errors go to the local server, which sends them
// only when the user agreed (and never from a checkout).
export function report(kind, body) {
  fetch('/api/telemetry/' + kind, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body), keepalive: true,
  }).catch(() => {});
}

export function showConsent(t) {
  const on = !!(t && t.available);
  $('consent-card').classList.toggle('hidden', !on || t.asked);
  $('privacy-off').classList.toggle('hidden', on);
  $('privacy-host').classList.toggle('hidden', !on);
  for (const [id, key] of [['set-crashes', 'crashes'], ['set-usage', 'usage']]) {
    $(id).disabled = !on;
    $(id).checked = on && t[key];
  }
}

async function saveConsent(usage, crashes) {
  const d = await postJSON('/api/telemetry/consent', { usage, crashes }).then(r => r.json());
  showConsent(d);
  if (usage) report('page', { page: currentSpace });
  return d;
}

$('consent-save').addEventListener('click', () =>
  saveConsent($('consent-usage').checked, $('consent-crashes').checked));
$('consent-none').addEventListener('click', () => saveConsent(false, false));
for (const id of ['set-crashes', 'set-usage'])
  $(id).addEventListener('change', async () => {
    await saveConsent($('set-usage').checked, $('set-crashes').checked);
    setMsg($('set-msg'), 'Saved', 'ok');
  });

onShow((name, prev) => { if (name !== prev) report('page', { page: name }); });

window.addEventListener('error', e => report('error', {
  message: e.message || String(e.error),
  stack: (e.error && e.error.stack) || `${e.filename}:${e.lineno}:${e.colno}`,
  page: currentSpace || '',
}));
window.addEventListener('unhandledrejection', e => report('error', {
  message: 'unhandled rejection: ' + ((e.reason && e.reason.message) || String(e.reason)),
  stack: (e.reason && e.reason.stack) || '',
  page: currentSpace || '',
}));
