// Crash reports / usage statistics (pakon-web telemetry.rs): the consent
// is asked once, Settings changes it.
import { postJSON, report } from './api';
import { app, type Telemetry } from './state';

export async function saveConsent(usage: boolean, crashes: boolean): Promise<Telemetry> {
  const d: Telemetry = await postJSON('/api/telemetry/consent', { usage, crashes }).then(r => r.json());
  if (app.status) app.status.telemetry = d;
  if (usage) report('page', { page: app.space });
  return d;
}

export function reportErrors() {
  window.addEventListener('error', e => report('error', {
    message: e.message || String(e.error),
    stack: (e.error && e.error.stack) || `${e.filename}:${e.lineno}:${e.colno}`,
    page: app.space,
  }));
  window.addEventListener('unhandledrejection', e => report('error', {
    message: 'unhandled rejection: ' + ((e.reason && e.reason.message) || String(e.reason)),
    stack: (e.reason && e.reason.stack) || '',
    page: app.space,
  }));
}
