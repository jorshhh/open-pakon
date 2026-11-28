// The roll: its raw, processing it (which opens Frames), the boxes on it,
// and the ways in (a scan, a raw opened, the work folder).
import { reactive } from 'vue';
import { postJSON, postSSE } from '../api';
import { app, browse, msg, remembered, setBusy, showSpace, splitPath, useWorkDir } from './app';
import { editFrom, type Edit } from './edits';
import { resetExport } from './exported';

export type Film = 'c41' | 'bw' | 'bwc41' | 'slide';
// The film types, in the order the menus list them.
export const FILMS: readonly { id: Film; name: string }[] = [
  { id: 'c41', name: 'Colour negative' },
  { id: 'bw', name: 'B&W' },
  { id: 'bwc41', name: 'B&W C41' },
  { id: 'slide', name: 'Slide (E-6)' },
];
export type ColorEngine = 'psi3' | 'psi24';
export const COLOR_ENGINES: readonly { id: ColorEngine; name: string }[] = [
  { id: 'psi3', name: 'PSI 3' },
  { id: 'psi24', name: 'PSI 2.4' },
];
const COLOR_ENGINE_IDS = ['psi3', 'psi24'] as const;
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
  saved?: { centres: number[]; widths?: number[]; edits?: Record<string, number | boolean>[]; color_engine?: ColorEngine } | null;
  strips?: Strip[];      // strip mode: the strips in film order
  dx_order?: boolean;    // put in that order from their DX codes
  first_frame?: number | null;  // row where the first picture found starts
  adjustable?: boolean;  // colour edits and sharpening render (OEM or slide path)
}
// One cut strip on the ribbon: full rows [start, end), fed = its place in the feed.
export interface Strip { start: number; end: number; fed: number }
// A box: centre row, width (full res), and Adjust's edits on it.
export interface Crop { c: number; w: number; e?: Edit }

export const roll = reactive({
  path: '',
  dx: null as Dx | null,
  film: 'c41' as Film,
  colorEngine: remembered('pakon-color-engine', 'psi3', COLOR_ENGINE_IDS) as ColorEngine,
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

export function showRaw(path: string, dx?: Dx | null, film?: Film | null) {
  roll.path = path;
  roll.dx = dx ?? null;
  if (film) roll.film = film;
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

// A processed roll: the boxes (and their edits) saved at its last export,
// else none.
function openRoll(m: RollMeta) {
  const saved = m.saved;
  roll.crops = saved ? saved.centres.map((c, i) => ({ c, w: saved.widths?.[i] || m.frame_w, e: editFrom(saved.edits?.[i]) })) : [];
  roll.colorEngine = m.film === 'slide'
    ? 'psi3'
    : saved
      ? (saved.color_engine === 'psi24' ? 'psi24' : 'psi3')
      : remembered('pakon-color-engine', 'psi3', COLOR_ENGINE_IDS);
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
