// Status polling: the scanner badge, which Scan cards show, and whatever
// else /api/status carries (work folder, new build, consent, version).
import { $ } from './util.js';
import { showWorkDir, setNativePicker } from './browse.js';
import { showUpdate } from './update.js';
import { showConsent } from './telemetry.js';

const labels = {
  warm: 'Scanner ready',
  cold: 'Scanner detected — needs firmware',
  disconnected: 'No scanner detected',
  unknown: 'Scanner status unknown',
};

async function pollStatus() {
  try {
    const d = await fetch('/api/status').then(r => r.json());

    // Badge
    $('badge').className = 'badge ' + d.state;
    $('dot').className   = 'dot '   + d.state;
    $('badge-text').textContent = d.state === 'unknown' && d.state_error
      ? `Status unknown — ${d.state_error}`
      : (labels[d.state] || d.state);

    $('fw-card').classList.toggle('hidden', d.state !== 'cold');
    $('scan-card').classList.toggle('hidden', d.state !== 'warm');
    $('scan-empty').classList.toggle('hidden', d.state === 'cold' || d.state === 'warm');
    $('scan-empty-msg').textContent = d.state === 'unknown' && d.state_error
      ? d.state_error
      : 'Connect the F-135 by USB and turn it on. It shows up here by itself.';
    $('home-dot').className = 'dot ' + d.state;
    $('home-state').textContent = labels[d.state] || d.state;

    $('scan-mode').textContent = d.eeprom
      ? `F-135: scan built in code (EEPROM ${d.eeprom}); F-135+: replayed script`
      : (d.eeprom_unit && d.eeprom_unit.error)
        ? `EEPROM: ${d.eeprom_unit.error} — scans are replayed`
        : 'No EEPROM archive yet — it is read when the scanner is warm; until then scans are replayed';

    showWorkDir(d.work_dir, d.next_order);
    setNativePicker(d.native_picker);
    showUpdate(d.update);
    showConsent(d.telemetry);
    if (d.version) $('about-version').textContent = d.version;
  } catch {
    $('home-dot').className = 'dot';
    $('home-state').textContent = 'Scanner status unknown';
  }
}

export function startPolling() {
  setInterval(pollStatus, 2000);
  pollStatus();
}
