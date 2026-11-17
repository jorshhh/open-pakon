// What the status poll says about the scanner, for the badge and Home.
import { computed } from 'vue';
import { app } from './state';

const labels: Record<string, string> = {
  warm: 'Ready',
  cold: 'Needs firmware',
  disconnected: 'No scanner',
  unknown: 'Status unknown',
};

// '' until the first answer. A missed poll keeps the last answer, so the
// Scan card (and its Stop button) stays through a hiccup.
export const scannerState = computed(() => app.status?.state ?? '');

// "F-135+ · #138 · Ready" once the EEPROM names the unit, else the state alone.
export const badgeText = computed(() => {
  const d = app.status;
  if (!d) return 'Checking…';
  if (d.state === 'unknown' && d.state_error) return 'Status unknown';
  const u = d.eeprom_unit || {};
  const label = labels[d.state] || d.state;
  return d.state === 'warm' && u.model && u.serial != null ? `${u.model} · #${u.serial} · ${label}` : label;
});

// Home's line, which also says when the service stopped answering.
export const homeState = computed(() => (app.statusFailed ? 'unknown' : scannerState.value));
export const homeText = computed(() => (app.statusFailed ? labels.unknown : badgeText.value));
