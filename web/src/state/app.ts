// The page around the spaces: which one is shown, the scanner's status,
// messages, the order and the folder browser, and what the browser
// remembers.
import { reactive, watch } from 'vue';
import { getJSON, postJSON, report, store } from '../api';

export type Space = 'home' | 'scan' | 'process' | 'frames' | 'adjust' | 'export' | 'settings';

// A host path's folder and file name, '/' or '\\' (Windows) separated.
export function splitPath(path: string): { dir: string; name: string } {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return { dir: cut < 0 ? '' : path.slice(0, cut), name: path.slice(cut + 1) };
}
export interface Msg { text: string; kind?: 'ok' | 'err'; title?: string }
export const msg = (text = '', kind?: Msg['kind'], title?: string): Msg => ({ text, kind, title });

export interface Telemetry { available: boolean; asked: boolean; usage: boolean; crashes: boolean }
export interface Update { latest: string; current: string; install?: boolean; url?: string; os?: 'macos' | 'windows' | 'linux' }
export interface Status {
  state: 'warm' | 'cold' | 'disconnected' | 'unknown' | string;
  state_error?: string;
  eeprom?: string | null;
  eeprom_unit?: { model?: string; serial?: number; error?: string } | null;
  work_dir?: string;
  next_order?: string;
  native_picker?: boolean;
  update?: Update | null;
  telemetry?: Telemetry;
  logs?: boolean;
  foreign_firmware?: boolean | null;
  version?: string;
}

export const app = reactive({
  space: 'home' as Space,
  busy: {} as Partial<Record<Space, boolean>>,
  status: null as Status | null,
  statusFailed: false,
  updateLater: false,
  scanMsg: msg(),
});

// The order the next scan goes into: '' = the counter's next number (#95).
export const order = reactive({ custom: '' });

// The page's folder browser (a plain browser; the app window has the system's).
export const browse = reactive({ open: false, raws: false });

export function showSpace(name: Space) { app.space = name; }
export function setBusy(name: Space, on: boolean) { app.busy[name] = on; }

// every space shown, the first one included
watch(() => app.space, (name, prev) => { if (name !== prev) report('page', { page: name }); }, { immediate: true });

// ── Status, every 2 s ────────────────────────────────────────────────────
async function pollStatus() {
  try {
    app.status = await getJSON<Status>('/api/status');
    app.statusFailed = false;
  } catch {
    app.statusFailed = true;
  }
}
export function startPolling() {
  pollStatus();
  setInterval(pollStatus, 2000);
}

export async function useWorkDir(path: string): Promise<string | null> {
  const resp = await postJSON('/api/workdir', { path });
  const d = await resp.json();
  if (!resp.ok) return d.error || 'could not set the work folder';
  if (app.status) Object.assign(app.status, { work_dir: d.work_dir, next_order: d.next_order });
  return null;
}

// Remembered in this browser.
export function remembered<T extends string>(key: string, fallback: T, ok: readonly T[]): T {
  const v = store.get(key) as T | null;
  return v && ok.includes(v) ? v : fallback;
}
