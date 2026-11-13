// Status polling: the scanner badge, which Scan cards show, and whatever
// else /api/status carries (work folder, new build, consent, version).
import { $ } from './util.js';
import { showWorkDir, setNativePicker } from './browse.js';
import { showUpdate } from './update.js';
import { showConsent } from './telemetry.js';
import { showLogs } from './support.js';

const labels = {
  warm: 'Ready',
  cold: 'Needs firmware',
  disconnected: 'No scanner',
  unknown: 'Status unknown',
};

// "F-135+ · #138 · Ready" once the EEPROM names the unit, else the state alone.
function badgeText(d) {
  if (d.state === 'unknown' && d.state_error) return 'Status unknown';
  const u = d.eeprom_unit || {};
  const label = labels[d.state] || d.state;
  return d.state === 'warm' && u.model && u.serial != null
    ? `${u.model} · #${u.serial} · ${label}` : label;
}

async function pollStatus() {
  try {
    const d = await fetch('/api/status').then(r => r.json());

    // Badge
    $('badge').className = 'badge ' + d.state;
    $('dot').className   = 'dot '   + d.state;
    $('badge-text').textContent = badgeText(d);
    $('badge').title = d.state_error || '';

    $('fw-card').classList.toggle('hidden', d.state !== 'cold');
    $('scan-card').classList.toggle('hidden', d.state !== 'warm');
    $('scan-empty').classList.toggle('hidden', d.state === 'cold' || d.state === 'warm');
    $('scan-empty-msg').textContent = d.state === 'unknown' && d.state_error
      ? d.state_error
      : 'Connect it by USB and turn it on.';
    $('home-dot').className = 'dot ' + d.state;
    $('home-state').textContent = badgeText(d);

    // Only the exception is worth a word: no calibration data, so the scan
    // is a replay.
    $('scan-mode').textContent = d.eeprom ? '' : 'Replayed scan';
    $('scan-mode').title = d.eeprom ? '' : (d.eeprom_unit && d.eeprom_unit.error)
      ? `EEPROM: ${d.eeprom_unit.error}`
      : 'The EEPROM is read once the scanner is warm; until then scans are replayed';

    showWorkDir(d.work_dir, d.next_order);
    setNativePicker(d.native_picker);
    showUpdate(d.update);
    showConsent(d.telemetry);
    showLogs(!!d.logs);
    if (d.version) $('about-version').textContent = d.version;
  } catch {
    $('home-dot').className = 'dot';
    $('home-state').textContent = labels.unknown;
  }
}

export function startPolling() {
  setInterval(pollStatus, 2000);
  pollStatus();
}
