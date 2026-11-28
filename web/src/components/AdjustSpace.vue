<script setup lang="ts">
// Adjust: colour, density, contrast, saturation, turn and flip per frame,
// sharpening and dust removal for the roll, on the boxes placed in Frames,
// with live previews; then the export at full resolution. Edits stay on
// the boxes (roll.crops[i].e) and are saved with them at export. The page
// fits the window: the frames shrink to fit, it never scrolls.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { postSSE, store } from '../api';
import { app, remembered, setBusy, showSpace } from '../state/app';
import { edited, editJSON, noEdit, type Edit, type Slider } from '../state/edits';
import { loadFrames, resetExport } from '../state/exported';
import { COLOR_ENGINES, roll, type ColorEngine } from '../state/roll';
import Btn from '../ui/Btn.vue';
import Card from '../ui/Card.vue';
import EmptyCard from '../ui/EmptyCard.vue';
import InfoTip from '../ui/InfoTip.vue';
import KeyHints from '../ui/KeyHints.vue';
import LatestImg from '../ui/LatestImg.vue';

const meta = computed(() => roll.meta);
const MOD = /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl';
const KEYS = [
  { keys: ['Click'], what: 'select' },
  { keys: ['Shift', 'Click'], what: 'a run' },
  { keys: [MOD, 'Click'], what: 'one more' },
  { keys: [MOD, 'A'], what: 'all' },
  { keys: ['←', '→'], what: 'previous / next' },
];
const n = computed(() => roll.crops.length);
// Colour edits and sharpening need the OEM colour path (the scanner's
// EEPROM archive and rpd.pf) or a slide; without them only turn and flip.
const adjustable = computed(() => !!meta.value?.adjustable || colorEngine.value === 'psi24');
const isSlide = computed(() => meta.value?.film === 'slide');
const colorEngine = computed<ColorEngine>(() => (isSlide.value ? 'psi3' : roll.colorEngine));
const shown = computed(() => app.space === 'adjust' && !!meta.value && n.value > 0);

// ── View: the whole roll, 4 or 2 at a time, or one ──────────────────────
const VIEWS = ['roll', '4', '2', '1'] as const;
type View = (typeof VIEWS)[number];
const view = ref<View>(remembered('pakon-adjust-view', 'roll', VIEWS));
watch(view, v => store.set('pakon-adjust-view', v));

// ── Fit to the window ────────────────────────────────────────────────────
// The frames' box runs from where it starts to the bottom bar; the frames
// are sized to fill it (3:2 cells, a picture letterboxed in its cell).
const area = ref<HTMLElement | null>(null);
const box = ref({ w: 800, h: 500 });
const GAP = 8, PAD = 10, STRIP = 72, ARROW = 54, STATUS = 36;
function measure() {
  const el = area.value;
  if (!el || !shown.value) return;
  const top = el.getBoundingClientRect().top + window.scrollY;
  const below = parseFloat(getComputedStyle(document.body).paddingBottom) || 0;
  // card padding + border, the status line under the frames
  const h = Math.max(320, Math.floor(window.innerHeight - top - below - 28 - STATUS));
  box.value = { w: el.clientWidth, h };
}
let ro: ResizeObserver | null = null;
onMounted(() => {
  window.addEventListener('resize', measure);
  window.addEventListener('keydown', onKey);
});
onBeforeUnmount(() => {
  window.removeEventListener('resize', measure);
  window.removeEventListener('keydown', onKey);
  ro?.disconnect();
});
watch(area, el => {
  ro?.disconnect();
  if (el) { ro = new ResizeObserver(measure); ro.observe(el); }
});
watch(shown, on => { if (on) nextTick(measure); }, { immediate: true });

// cells: c across, r down, cw × ch px each, fitted into w × h
function fit(c: number, r: number, w: number, h: number) {
  const cw = Math.max(40, Math.min((w - GAP * (c - 1)) / c, ((h - GAP * (r - 1)) / r) * 1.5));
  return { c, cw: Math.floor(cw), ch: Math.floor(cw / 1.5) };
}
const PER: Record<View, number> = { roll: 0, '4': 4, '2': 2, '1': 1 };
const grid = computed(() => {
  const w = box.value.w - 2 * PAD - 4, h = box.value.h - 2 * PAD - 4;
  if (view.value === 'roll') {
    let best = fit(1, n.value, w, h);
    for (let c = 2; c <= n.value; c++) {
      const g = fit(c, Math.ceil(n.value / c), w, h);
      if (g.cw > best.cw) best = g;
    }
    return best;
  }
  const hh = h - STRIP - GAP;
  if (view.value === '1') return fit(1, 1, w - 2 * ARROW, hh);
  return view.value === '4' ? fit(2, 2, w, hh) : fit(2, 1, w, hh);
});
// the frames shown: all of them, or the page holding the current one
const cur = computed(() => Math.min(roll.cur, Math.max(0, n.value - 1)));
const visible = computed(() => {
  const per = PER[view.value];
  if (!per) return roll.crops.map((_, i) => i);
  const start = Math.floor(cur.value / per) * per;
  return Array.from({ length: Math.min(per, n.value - start) }, (_, k) => start + k);
});
// the preview size the cells need: the kept 1000 px, or larger to fill them
const px = computed(() => {
  const v = Math.ceil(Math.max(grid.value.cw, 1) * (window.devicePixelRatio || 1));
  return v <= 1000 ? 1000 : Math.min(3000, Math.ceil(v / 500) * 500);
});

// ── Previews: the service renders the boxes small once, then any frame
// with any edit on request ────────────────────────────────────────────────
const boxKey = () => JSON.stringify(roll.crops.map(c => [Math.round(c.c), Math.round(c.w)]));
const prepKey = () => JSON.stringify({
  b: roll.crops.map(c => [Math.round(c.c), Math.round(c.w)]),
  e: colorEngine.value,
});
const opened = ref('');           // the boxes + colour engine the service's previews are of
const stamp = ref(0);
const status = ref({ busy: false, value: null as number | null, label: '', error: false });
const preparing = ref(false);
const prepFailed = ref(false);
const ready = computed(() => !!opened.value && opened.value === prepKey());

async function openPreviews() {
  const key = prepKey();
  if (preparing.value || key === opened.value) return;
  preparing.value = true;
  prepFailed.value = false;
  status.value = { busy: true, value: null, label: 'Preparing the previews', error: false };
  setBusy('adjust', true);
  await postSSE('/api/adjust/open', {
    centres: roll.crops.map(c => Math.round(c.c)),
    widths: roll.crops.map(c => Math.round(c.w)),
    color_engine: colorEngine.value,
  }, ev => {
    if (ev.type === 'progress') status.value = { busy: true, value: ev.pct, label: ev.step, error: false };
    else if (ev.type === 'done') { opened.value = key; stamp.value = Date.now(); status.value = { busy: false, value: null, label: '', error: false }; }
    else if (ev.type === 'error') {
      prepFailed.value = true;
      status.value = { busy: false, value: null, label: ev.message ?? 'Could not prepare the previews', error: true };
    }
  });
  preparing.value = false;
  setBusy('adjust', false);
  // the boxes or colour engine changed while these were made
  if (shown.value && !status.value.error && prepKey() !== opened.value) openPreviews();
}
watch(meta, () => { opened.value = ''; });                 // a new roll
watch(() => (shown.value ? prepKey() : null), k => { if (k) openPreviews(); }, { immediate: true });

function src(i: number, size: number, colour = true) {
  const q = new URLSearchParams(Object.entries(editJSON(roll.crops[i]?.e, colour)).map(([k, v]) => [k, String(v)]));
  q.set('px', String(size));
  if (colour && sharpen.value !== 'off') q.set('sharpen', sharpen.value);
  q.set('t', String(stamp.value));
  return `/api/adjust/${i + 1}?${q}`;
}

// ── Selection: click one, Shift a run, ⌘/Ctrl one more, ⌘/Ctrl-A all ───
const sel = ref<number[]>([0]);
// a selection is of positions: boxes added, removed or moved in Frames
// start it over at the current frame
watch(boxKey, () => { sel.value = [cur.value]; });
function pick(i: number, e?: MouseEvent) {
  if (e?.shiftKey && sel.value.length) {
    const lo = Math.min(roll.cur, i), hi = Math.max(roll.cur, i);
    sel.value = Array.from({ length: hi - lo + 1 }, (_, k) => lo + k);
  } else if (e && (e.metaKey || e.ctrlKey)) {
    sel.value = sel.value.includes(i) ? sel.value.filter(s => s !== i) : [...sel.value, i];
    if (!sel.value.length) sel.value = [i];
  } else {
    sel.value = [i];
  }
  roll.cur = i;
}
function go(d: number) { pick(Math.max(0, Math.min(n.value - 1, cur.value + d))); }
function selectAll() { sel.value = roll.crops.map((_, i) => i); }
const panelTitle = computed(() => (sel.value.length === 1 ? `Frame ${sel.value[0] + 1}` : `${sel.value.length} frames`));

// the strip under the page views keeps the current frame in sight
const strip = ref<HTMLElement | null>(null);
watch(cur, i => nextTick(() => strip.value?.children[i]?.scrollIntoView({ block: 'nearest', inline: 'nearest' })));

// ── The controls ─────────────────────────────────────────────────────────
// Colour pairs as their colours; the others as − / + in ink.
const ROWS: { k: Slider; name: string; lo?: string; hi?: string; less: string; more: string; max: number }[] = [
  { k: 'cr', name: 'C · R', lo: '#00aeef', hi: '#e53935', less: 'More cyan', more: 'More red', max: 20 },
  { k: 'mg', name: 'M · G', lo: '#e6007e', hi: '#2e9e44', less: 'More magenta', more: 'More green', max: 20 },
  { k: 'yb', name: 'Y · B', lo: '#ffd500', hi: '#1e5bd8', less: 'More yellow', more: 'More blue', max: 20 },
  { k: 'dn', name: 'Density', less: 'Lighter', more: 'Darker', max: 20 },
  { k: 'ct', name: 'Contrast', less: 'Less contrast', more: 'More contrast', max: 20 },
  { k: 'sat', name: 'Saturation', less: 'Less saturation', more: 'More saturation', max: 5 },
];
// the selection's value, or null when the frames differ
function value(k: Slider): number | null {
  const vs = sel.value.map(i => roll.crops[i]?.e?.[k] ?? 0);
  return vs.every(v => v === vs[0]) ? vs[0] ?? 0 : null;
}
const rows = computed(() => ROWS.map(r => {
  const v = value(r.k);
  return { ...r, v, text: v == null ? '—' : v > 0 ? `+${v}` : String(v), pos: v == null ? 50 : 50 + (50 * v) / r.max };
}));

function edit(fn: (e: Edit) => void) {
  for (const i of sel.value) {
    const c = roll.crops[i];
    if (!c) continue;
    const e = c.e ? { ...c.e } : noEdit();
    fn(e);
    c.e = edited(e) ? e : undefined;
  }
}
function step(r: (typeof ROWS)[number], d: number) {
  edit(e => { e[r.k] = Math.max(-r.max, Math.min(r.max, e[r.k] + d)); });
}
const turn = (deg: number) => edit(e => { e.rot = (((e.rot + deg) % 360) + 360) % 360; });
const flipH = () => edit(e => { e.fh = !e.fh; });
const flipV = () => edit(e => { e.fv = !e.fv; });
// The sliders: a click or drag sets the value under the pointer.
type Row = (typeof ROWS)[number];
let sliding: Slider | null = null;
function valueAt(r: Row, e: PointerEvent) {
  const b = (e.currentTarget as HTMLElement).getBoundingClientRect();
  const f = (e.clientX - b.left) / Math.max(1, b.width);
  return Math.max(-r.max, Math.min(r.max, Math.round((f * 2 - 1) * r.max)));
}
function setTo(r: Row, v: number) { edit(e => { e[r.k] = v; }); }
function slideStart(r: Row, e: PointerEvent) {
  if (e.button !== 0) return;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  sliding = r.k;
  setTo(r, valueAt(r, e));
}
function slideMove(r: Row, e: PointerEvent) {
  if (sliding !== r.k) return;
  const v = valueAt(r, e);
  if (v !== value(r.k)) setTo(r, v);
}
function slideEnd() { sliding = null; }
function slideKey(r: Row, e: KeyboardEvent) {
  const k = e.key;
  if (k === 'ArrowLeft' || k === 'ArrowDown') step(r, -1);
  else if (k === 'ArrowRight' || k === 'ArrowUp') step(r, 1);
  else if (k === 'Home') setTo(r, -r.max);
  else if (k === 'End') setTo(r, r.max);
  else return;
  e.preventDefault();
}
function reset() { for (const i of sel.value) if (roll.crops[i]) roll.crops[i].e = undefined; }

// One view: hold to see the frame without its colour edits.
const original = ref(false);

// keyboard: ← → previous / next frame, ⌘/Ctrl-A every frame
function onKey(e: KeyboardEvent) {
  if (!shown.value || e.defaultPrevented) return;
  const t = e.target as HTMLElement | null;
  if (t?.closest?.('select, input, textarea, [role="dialog"], [role="slider"]')) return;
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'a') selectAll();
  else if (e.metaKey || e.ctrlKey || e.altKey) return;
  else if (e.key === 'ArrowLeft') go(-1);
  else if (e.key === 'ArrowRight') go(1);
  else return;
  e.preventDefault();
}

// ── Export: dust removal, then every box at full resolution ─────────────
// Dust removal needs the scan's infrared channel, and older raws may lack
// the calibration's IR table. Silver B&W gets none (silver blocks IR as dust
// does): the prescan leaves meta.ice off for it. On by default wherever it
// can run; turning it off is remembered in this browser.
const iceOk = computed(() => !!meta.value?.ice);
const iceTip = computed(() => iceOk.value
  ? 'Digital ICE: rebuilds dust and scratches from the infrared channel. Slower export.'
  : meta.value?.film === 'bw'
  ? 'Off for silver B&W: the silver blocks infrared just as dust does.'
  : 'This scan has no infrared channel to find dust with.');
const ice = ref(store.get('pakon-ice') !== '0');
watch(ice, v => store.set('pakon-ice', v ? '1' : '0'));
// Sharpening for the whole roll: the OEM's unsharp mask at 1 / 2 / 3 TLX
// clicks; shown on the previews too.
const SHARPEN = ['off', 'low', 'medium', 'high'] as const;
const sharpen = ref(remembered('pakon-sharpen', 'off', SHARPEN));
watch(sharpen, v => store.set('pakon-sharpen', v));

function engineChanged() { store.set('pakon-color-engine', roll.colorEngine); }

const exporting = ref(false);
async function exportFrames() {
  exporting.value = true;
  setBusy('adjust', true);
  status.value = { busy: true, value: 0, label: 'Exporting', error: false };
  resetExport();
  await postSSE('/api/export_frames', {
    centres: roll.crops.map(c => Math.round(c.c)),
    widths: roll.crops.map(c => Math.round(c.w)),
    edits: roll.crops.map(c => editJSON(c.e)),
    ice: ice.value && iceOk.value,
    sharpen: sharpen.value,
    color_engine: colorEngine.value,
  }, ev => {
    if (ev.type === 'progress') status.value = { busy: true, value: ev.pct, label: ev.step, error: false };
    else if (ev.type === 'done') {
      status.value = { busy: true, value: 1, label: 'Done', error: false };
      loadFrames().then(() => { status.value = { busy: false, value: null, label: '', error: false }; showSpace('export'); });
    } else if (ev.type === 'error') status.value = { busy: false, value: null, label: ev.message ?? 'Export failed', error: true };
  });
  exporting.value = false;
  setBusy('adjust', false);
}

const btnCls = 'grid size-11 shrink-0 cursor-pointer place-items-center border-2 border-ink bg-surface hover:bg-accent';
// the panel's turn and flip buttons: square, sized to the window height
const toolCls = 'grid size-[clamp(1.75rem,5vh,2.5rem)] shrink-0 cursor-pointer place-items-center border-2 border-ink bg-surface hover:bg-accent';
</script>

<template>
  <EmptyCard v-if="!meta" title="No roll open">Scan a roll or open a raw.</EmptyCard>
  <EmptyCard v-else-if="!n" title="No frames yet">Place them in <b>Frames</b>.</EmptyCard>

  <Card v-else ruled class="mb-0!">
    <template #title>Adjust {{ n }} frames</template>
    <template #tools>
      <div class="flex flex-wrap items-center gap-3 text-body">
        <div role="group" aria-label="View" class="flex border-2 border-ink">
          <button v-for="v in VIEWS" :key="v" type="button" :aria-pressed="view === v" @click="view = v"
                  :class="['min-h-9 min-w-11 cursor-pointer border-r-2 border-ink px-3 font-mono text-small font-bold uppercase last:border-r-0',
                           view === v ? 'bg-ink text-surface' : 'bg-surface text-ink hover:bg-paper']">{{ v }}</button>
        </div>
        <Btn variant="ghost" @click="selectAll">Select all</Btn>
        <Btn :disabled="exporting || preparing" @click="exportFrames">Export {{ n }} frames</Btn>
      </div>
    </template>

    <div class="flex items-start gap-4">
      <!-- the frames, fitted to the window -->
      <div ref="area" class="flex min-w-0 flex-1 flex-col gap-2 overflow-hidden border-2 border-ink bg-ink p-2.5" :style="{ height: box.h + 'px' }">
        <div class="relative flex min-h-0 flex-1 items-center justify-center gap-2.5">
          <button v-if="view === '1'" type="button" aria-label="Previous frame" :class="btnCls + ' border-surface! bg-ink! text-surface hover:bg-ink!'" @click="go(-1)">
            <svg class="size-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="m15 6-6 6 6 6"/></svg>
          </button>
          <div class="grid content-center justify-center gap-2" :style="{ gridTemplateColumns: `repeat(${grid.c}, ${grid.cw}px)`, gridAutoRows: grid.ch + 'px' }">
            <button v-for="i in visible" :key="i" type="button" :aria-label="`Frame ${i + 1}`" :aria-pressed="sel.includes(i)"
                    @click="pick(i, $event)" @dblclick="pick(i); view = '1'"
                    :class="['relative cursor-pointer overflow-hidden bg-black p-0', sel.includes(i) ? 'border-3 border-danger' : 'border-2 border-accent/80']">
              <LatestImg v-if="ready" :src="src(i, px, !(view === '1' && original))" :alt="`Frame ${i + 1}`" class="size-full object-contain" />
              <span :class="['absolute top-1 left-1 border border-black px-1.5 font-mono text-tiny font-bold', sel.includes(i) ? 'bg-danger text-white' : 'bg-accent text-black']">{{ i + 1 }}</span>
              <span v-if="edited(roll.crops[i]?.e)" class="absolute top-1 right-1 size-2.5 border border-black bg-accent" title="Adjusted"></span>
            </button>
          </div>
          <button v-if="view === '1'" type="button" aria-label="Next frame" :class="btnCls + ' border-surface! bg-ink! text-surface hover:bg-ink!'" @click="go(1)">
            <svg class="size-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="m9 6 6 6-6 6"/></svg>
          </button>
          <button v-if="view === '1'" type="button" class="absolute right-0 bottom-0 min-h-10 cursor-pointer border-2 border-ink bg-surface px-3.5 font-mono text-small font-bold select-none"
                  @pointerdown="original = true" @pointerup="original = false" @pointerleave="original = false" @blur="original = false">Hold: original</button>
        </div>
        <!-- the roll under the page views -->
        <div v-if="view !== 'roll'" ref="strip" class="flex shrink-0 gap-1.5 overflow-x-auto [scrollbar-color:var(--color-muted)_transparent] [scrollbar-width:thin]" :style="{ height: STRIP - GAP + 'px' }" aria-label="Roll">
          <button v-for="(_, i) in roll.crops" :key="i" type="button" :aria-label="`Frame ${i + 1}`" @click="pick(i, $event)"
                  :class="['relative aspect-[3/2] h-full shrink-0 cursor-pointer overflow-hidden bg-black p-0', sel.includes(i) ? 'border-3 border-danger' : 'border-2 border-accent/80']">
            <LatestImg v-if="ready" :src="src(i, 1000)" class="size-full object-contain" />
            <span :class="['absolute top-0.5 left-0.5 border border-black px-1 font-mono text-tiny font-bold', sel.includes(i) ? 'bg-danger text-white' : 'bg-accent text-black']">{{ i + 1 }}</span>
          </button>
        </div>
      </div>

      <!-- the right column: the roll's settings, then the frame's controls;
           together as tall as the frames, so neither ever scrolls -->
      <div class="flex w-80 shrink-0 flex-col gap-3" :style="{ height: box.h + 'px' }">
        <!-- for the whole roll -->
        <div class="flex shrink-0 flex-wrap items-center justify-between gap-x-3 gap-y-1.5 border-2 border-ink bg-paper px-3 py-2 text-body">
          <label class="flex items-center gap-2 font-mono font-semibold">Sharpen
            <select v-model="sharpen" aria-label="Sharpening" :disabled="!adjustable">
              <option value="off">Off</option>
              <option value="low">Low</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
            </select>
          </label>
          <span class="flex items-center gap-1.5">
            <label :class="['inline-flex items-center gap-1.5 whitespace-nowrap font-mono font-semibold', iceOk ? 'cursor-pointer' : 'cursor-not-allowed opacity-45']">
              <input type="checkbox" :checked="ice && iceOk" :disabled="!iceOk"
                     @change="ice = ($event.target as HTMLInputElement).checked"> Remove dust</label>
            <InfoTip :tip="iceTip" label="About dust removal" place="left" />
          </span>
          <label v-if="!isSlide" class="flex w-full items-center justify-between gap-2 font-mono font-semibold">
            <span class="flex items-center gap-1.5">Colour
              <InfoTip tip="PSI 3: Kodak Ansel (default). PSI 2.4: classic Pakon KCDFS colour and scene balance." label="About colour engine" place="left" />
            </span>
            <select v-model="roll.colorEngine" aria-label="Colour engine" :disabled="exporting || preparing"
                    @change="engineChanged">
              <option v-for="e in COLOR_ENGINES" :key="e.id" :value="e.id">{{ e.name }}</option>
            </select>
          </label>
        </div>
      <aside aria-label="Adjustments" class="grid min-h-0 flex-1 content-start border-2 border-ink bg-paper p-3"
             :style="{ gridTemplateRows: 'auto repeat(6, minmax(1.25rem, 2.25rem)) auto', rowGap: 'clamp(3px, 1.2vh, 12px)' }">
        <div class="flex items-center justify-between">
          <span class="flex items-center gap-2">
            <h3 class="font-display text-heading font-bold uppercase">{{ panelTitle }}</h3>
            <InfoTip v-if="!adjustable" label="Why colour is off" place="below"
                     tip="Colour and sharpening need the scanner's saved settings, which this roll has none of. Turn and flip still work." />
          </span>
          <Btn variant="ghost" class="px-2.5! py-1! text-small!" @click="reset">Reset</Btn>
        </div>
        <div v-for="r in rows" :key="r.k" :aria-disabled="!adjustable"
             :class="['grid min-h-0 grid-cols-[5.5rem_2.5rem_minmax(0,1fr)_2.5rem_2.25rem] items-stretch gap-1.5 font-mono text-small', !adjustable && 'pointer-events-none opacity-40']">
          <span class="self-center font-bold">{{ r.name }}</span>
          <button type="button" :aria-label="r.less" :title="r.less" class="grid cursor-pointer place-items-center border-2 border-ink bg-surface hover:bg-paper active:translate-y-px" :disabled="!adjustable" @click="step(r, -1)">
            <svg class="size-4" viewBox="0 0 12 12" aria-hidden="true"><path d="M1 2.5h10L6 10.5z" :fill="r.lo ?? 'currentColor'" stroke="#141414" stroke-width="1" stroke-linejoin="round"/></svg>
          </button>
          <!-- click or drag to set, arrows to step, double-click for 0 -->
          <div role="slider" :tabindex="adjustable ? 0 : -1" :aria-label="r.name" :aria-valuemin="-r.max" :aria-valuemax="r.max"
               :aria-valuenow="r.v ?? undefined" :aria-valuetext="r.text"
               class="relative cursor-pointer touch-none outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
               @pointerdown="slideStart(r, $event)" @pointermove="slideMove(r, $event)" @pointerup="slideEnd" @pointercancel="slideEnd"
               @keydown="slideKey(r, $event)" @dblclick="setTo(r, 0)">
            <div class="pointer-events-none absolute inset-x-0 top-1/2 h-2.5 -translate-y-1/2 border-2 border-ink bg-surface">
              <span class="absolute -inset-y-1 left-1/2 w-0.5 bg-muted"></span>
              <span :class="['absolute -inset-y-1.5 -ml-1 w-2 border border-ink', r.v == null ? 'bg-muted' : 'bg-ink']" :style="{ left: r.pos + '%' }"></span>
            </div>
          </div>
          <button type="button" :aria-label="r.more" :title="r.more" class="grid cursor-pointer place-items-center border-2 border-ink bg-surface hover:bg-paper active:translate-y-px" :disabled="!adjustable" @click="step(r, 1)">
            <svg class="size-4" viewBox="0 0 12 12" aria-hidden="true"><path d="M6 1.5l5 8H1z" :fill="r.hi ?? 'currentColor'" stroke="#141414" stroke-width="1" stroke-linejoin="round"/></svg>
          </button>
          <span class="self-center text-right font-bold">{{ r.text }}</span>
        </div>
        <div class="flex gap-2 border-t-2 border-ink pt-2.5">
          <button type="button" aria-label="Rotate left" title="Rotate left" :class="toolCls" @click="turn(-90)">
            <svg class="size-[clamp(0.875rem,2.6vh,1.25rem)]" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/></svg>
          </button>
          <button type="button" aria-label="Rotate right" title="Rotate right" :class="toolCls" @click="turn(90)">
            <svg class="size-[clamp(0.875rem,2.6vh,1.25rem)]" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 4v5h-5"/></svg>
          </button>
          <button type="button" aria-label="Flip horizontal" title="Flip horizontal" :class="toolCls" @click="flipH">
            <svg class="size-[clamp(0.875rem,2.6vh,1.25rem)]" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M12 3v18"/><path d="M8 7 3 12l5 5"/><path d="m16 7 5 5-5 5"/></svg>
          </button>
          <button type="button" aria-label="Flip vertical" title="Flip vertical" :class="toolCls" @click="flipV">
            <svg class="size-[clamp(0.875rem,2.6vh,1.25rem)]" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M3 12h18"/><path d="M7 8l5-5 5 5"/><path d="m7 16 5 5 5-5"/></svg>
          </button>
        </div>
      </aside>
      </div>
    </div>

    <!-- one line for the keys, progress and errors, so nothing moves -->
    <div class="mt-2 flex h-7 items-center gap-3 overflow-hidden font-mono text-small">
      <KeyHints :hints="KEYS" class="mr-auto" />
      <template v-if="status.busy">
        <progress v-if="status.value == null" class="w-48"></progress>
        <progress v-else :value="status.value" max="1" class="w-48"></progress>
      </template>
      <span :class="['truncate', status.error ? 'font-semibold text-danger' : 'text-muted']">{{ status.label }}</span>
      <Btn v-if="prepFailed && !preparing" variant="ghost" class="shrink-0 px-2.5! py-0.5! text-small!" @click="openPreviews">Retry</Btn>
    </div>
  </Card>
</template>
