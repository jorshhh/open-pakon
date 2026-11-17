// The page's shared state and the actions that cross spaces (a scan opens
// its raw in Process, processing opens Frames, export fills Export). Each
// space's own details stay in its component.
import { reactive, watch } from 'vue';
import { getJSON, postJSON, postSSE, report, store } from './api';

export type Space = 'home' | 'scan' | 'process' | 'frames' | 'export' | 'settings';
export type Film = 'c41' | 'bw' | 'bwc41' | 'slide';
// The film types, in the order the menus list them.
export const FILMS: readonly { id: Film; name: string }[] = [
  { id: 'c41', name: 'Colour negative' },
  { id: 'bw', name: 'B&W' },
  { id: 'bwc41', name: 'B&W C41' },
  { id: 'slide', name: 'Slide (E-6)' },
];

// A host path's folder and file name, '/' or '\\' (Windows) separated.
export function splitPath(path: string): { dir: string; name: string } {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return { dir: cut < 0 ? '' : path.slice(0, cut), name: path.slice(cut + 1) };
}
export interface Msg { text: string; kind?: 'ok' | 'err'; title?: string }
export const msg = (text = '', kind?: Msg['kind'], title?: string): Msg => ({ text, kind, title });

export interface Telemetry { available: boolean; asked: boolean; usage: boolean; crashes: boolean }
export interface Update { latest: string; current: string; install?: boolean; url?: string }
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
export interface Dx { name?: string; iso: number }

// The prescan's `done` event: the long preview and what framing needs.
export interface RollMeta {
  rows: number;          // full-resolution rows along the strip
  scale: number;         // preview px per full row
  preview_w: number;
  preview_h: number;
  frame_w: number;
  pitch: number;
  box_w?: Record<string, number>;
  format?: string;
  film?: Film;
  ice?: boolean;
  saved?: { centres: number[]; widths?: number[] } | null;
}
export interface Crop { c: number; w: number }   // centre row, width (full res)

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

// The roll: its raw, processing it, the boxes on it.
export const roll = reactive({
  path: '',
  dx: null as Dx | null,
  film: 'c41' as Film,
  processing: false,
  pct: 0,
  step: '',
  procMsg: msg(),
  // Frames' empty/progress card (pct null hides the bar)
  wait: { title: 'No roll open', msg: 'Scan a roll or open a raw.', pct: null as number | null },
  meta: null as RollMeta | null,
  crops: [] as Crop[],
  cur: 0,
  framesError: '',
  previewStamp: 0,
});

// What Export shows after an export.
export const exported = reactive({
  frames: [] as { index: number }[],
  folder: '',
  reveal: '',          // the service's name for "open the folder" ('' = cannot)
  stamp: 0,
});

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

// ── Process: the prescan of the open raw, which opens it in Frames ──────
export function showRaw(path: string, dx?: Dx | null, film?: Film | null) {
  roll.path = path;
  roll.dx = dx ?? null;
  if (film) roll.film = film;
}

export function resetExport() {
  exported.frames = [];
}

// A raw that comes in while another is processing (a scan ends, a raw is
// opened) is processed right after it; the one in between is not shown.
let again = false;

export async function processRaw() {
  if (roll.processing) { again = true; return; }
  again = false;
  roll.processing = true;
  setBusy('process', true);
  setBusy('frames', true);
  roll.procMsg = msg();
  roll.pct = 0;
  roll.step = '';
  roll.meta = null;
  roll.framesError = '';
  roll.wait = { title: 'Processing ' + splitPath(roll.path).name, msg: 'Starting…', pct: 0 };
  resetExport();

  await postSSE('/api/process', { film: roll.film }, ev => {
    if (ev.type === 'progress') {
      roll.pct = ev.pct;
      roll.step = ev.step;
      roll.wait = { ...roll.wait, msg: ev.step, pct: ev.pct };
    } else if (ev.type === 'done') {
      if (again) return;              // a newer raw is waiting
      roll.procMsg = msg('Ready in Frames', 'ok');
      openRoll(ev as unknown as RollMeta);
      showSpace('frames');
    } else if (ev.type === 'error') {
      if (again) return;
      roll.procMsg = msg(ev.message, 'err');
      roll.wait = { title: 'Could not process the roll', msg: ev.message ?? '', pct: null };
    }
  });
  roll.processing = false;
  if (again) { processRaw(); return; }
  setBusy('process', false);
  setBusy('frames', false);
}

// A processed roll: the boxes saved at its last export, else none.
function openRoll(m: RollMeta) {
  const saved = m.saved;
  roll.crops = saved ? saved.centres.map((c, i) => ({ c, w: saved.widths?.[i] || m.frame_w })) : [];
  roll.cur = 0;
  roll.previewStamp = Date.now();
  roll.meta = m;
}

// Open a raw where it is on the host and process it in place.
export async function openRaw(path: string): Promise<string | null> {
  const resp = await postJSON('/api/open', { path });
  const d = await resp.json();
  if (!resp.ok) return d.error || 'could not open';
  showRaw(d.path, d.dx, d.film);
  browse.open = false;
  showSpace('process');      // the progress is there; Frames opens when done
  processRaw();
  return null;
}

export async function useWorkDir(path: string): Promise<string | null> {
  const resp = await postJSON('/api/workdir', { path });
  const d = await resp.json();
  if (!resp.ok) return d.error || 'could not set the work folder';
  if (app.status) Object.assign(app.status, { work_dir: d.work_dir, next_order: d.next_order });
  return null;
}

// The ways in: the work folder (Scan), a raw (Process, Home). The system's
// dialog in the app window (/api/pick), else the page's folder browser.
export async function pick(raws: boolean) {
  if (!app.status?.native_picker) {
    Object.assign(browse, { open: true, raws });
    return;
  }
  const d = await postJSON('/api/pick', { kind: raws ? 'raw' : 'folder', start: app.status.work_dir })
    .then(r => r.json()).catch(() => ({}));
  if (!d.path) return;       // cancelled
  if (raws) {
    const err = await openRaw(d.path);
    if (err) { showSpace('process'); roll.procMsg = msg(err, 'err'); }
  } else {
    const err = await useWorkDir(d.path);
    if (err) app.scanMsg = msg(err, 'err');
  }
}

// The export's frames, for Export.
export async function loadFrames() {
  const d = await getJSON<{ folder: string; frames: { index: number }[]; reveal?: string }>('/api/frames');
  exported.reveal = d.reveal || '';
  exported.folder = d.folder || '';
  exported.stamp = Date.now();
  exported.frames = d.frames;
}

// Remembered in this browser.
export function remembered<T extends string>(key: string, fallback: T, ok: readonly T[]): T {
  const v = store.get(key) as T | null;
  return v && ok.includes(v) ? v : fallback;
}
