<script setup lang="ts">
// Frames: place the crop boxes on the film strip (minilab style), then
// export them at full resolution.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { postJSON, postSSE, store } from '../api';
import { app, loadFrames, resetExport, roll, setBusy, showSpace } from '../state';
import Btn from '../ui/Btn.vue';
import Card from '../ui/Card.vue';
import InfoTip from '../ui/InfoTip.vue';
import ProgressBar from '../ui/ProgressBar.vue';

const meta = computed(() => roll.meta);
const shown = computed(() => app.space === 'frames' && !!meta.value);

// ── Export options: dust removal and sharpening ──────────────────────────
// Dust removal needs the scan's infrared channel, and older raws may lack
// the calibration's IR table. Silver B&W gets none (silver blocks IR as dust
// does) and slides none (it softens their grain): the prescan leaves
// meta.ice off for both.
// The choices are remembered in this browser.
const iceOk = computed(() => !!meta.value?.ice);
const iceTip = computed(() => iceOk.value
  ? 'Digital ICE: rebuilds dust and scratches from the infrared channel. Slower export.'
  : meta.value?.film === 'bw'
  ? 'Off for silver B&W: the silver blocks infrared just as dust does.'
  : meta.value?.film === 'slide'
  ? 'Off for slides: it softens slide grain.'
  : 'This scan has no infrared channel to find dust with.');
const ice = ref(store.get('pakon-ice') === '1');
const sharpen = ref(store.get('pakon-sharpen') || 'off');
watch(ice, v => store.set('pakon-ice', v ? '1' : '0'));
watch(sharpen, v => store.set('pakon-sharpen', v));

// ── The strip ────────────────────────────────────────────────────────────
const format = ref('full');
const wrap = ref<HTMLElement | null>(null);
const inner = ref<HTMLElement | null>(null);
const zoom = ref(1);              // strip px per preview px

watch(meta, m => { if (m) format.value = m.format || 'full'; });

// The strip is drawn `zoom` times the preview JPEG, so it fills most of the
// window height whatever the preview's own size. It starts at its largest
// and shrinks by whatever then runs past the window, so Frames fits without
// scrolling.
async function fitStrip() {
  const m = meta.value;
  if (!m || !shown.value) return;
  const MIN = 160, MAX = 560;
  zoom.value = MAX / m.preview_h;
  await nextTick();
  const over = document.documentElement.scrollHeight - window.innerHeight;
  if (over > 0) zoom.value = Math.max(MIN, MAX - over) / m.preview_h;
  await nextTick();
  keepInView();
  wrap.value?.focus({ preventScroll: true });
}
// measured once it is on screen: a new roll, or coming back to Frames
watch(() => (shown.value ? meta.value : null), m => { if (m) fitStrip(); });
const onResize = () => { if (shown.value) fitStrip(); };

const pxPerRow = () => (meta.value ? meta.value.scale * zoom.value : 1);   // full row -> strip px
const stripW = computed(() => (meta.value ? Math.round(meta.value.preview_w * zoom.value) : 0));
const stripH = computed(() => (meta.value ? Math.round(meta.value.preview_h * zoom.value) : 0));

// Box width (full-res rows) for the format selected. The service sizes each
// format for the scan's resolution (box_w; at Base 16 full 3000, half 1500,
// panoramic 5530).
const boxSize = () => meta.value?.box_w?.[format.value] || meta.value?.frame_w || 0;

const boxes = computed(() => roll.crops.map((cp, i) => {
  const w = cp.w * pxPerRow();
  return { i, left: cp.c * pxPerRow() - w / 2, width: w };
}));
const active = computed(() => boxes.value[roll.cur] ?? null);

// Strip mode: where each cut strip starts on the film strip.
const strips = computed(() => meta.value?.strips ?? []);
const stripMarks = computed(() => strips.value.map((st, i) => ({ i, fed: st.fed, left: st.start * pxPerRow() })));
const title = computed(() => roll.crops.length ? `Confirm frames — ${roll.cur + 1} / ${roll.crops.length}` : 'Place the frames');

// keep the active box in view
function keepInView() {
  const w = wrap.value, cp = roll.crops[roll.cur];
  if (!w || !cp) return;
  const x = cp.c * pxPerRow();
  if (x < w.scrollLeft + 80 || x > w.scrollLeft + w.clientWidth - 80) w.scrollLeft = x - w.clientWidth / 2;
}
watch(() => [roll.cur, roll.crops.length], () => nextTick(keepInView));

function select(i: number) { roll.cur = i; }
function nudge(rows: number) {
  const m = meta.value, cp = roll.crops[roll.cur];
  if (!m || !cp) return;
  cp.c = Math.max(0, Math.min(m.rows, cp.c + rows));
  nextTick(keepInView);
}
const step = () => Math.max(4, Math.round((meta.value?.pitch ?? 0) * 0.004));   // micro (full rows)
function go(d: number) { roll.cur = Math.max(0, Math.min(roll.crops.length - 1, roll.cur + d)); }
// A new box of the selected size goes right after the last one (edge to
// edge), so full, half and panoramic frames tile when added in a run.
function addFrame() {
  const m = meta.value;
  if (!m) return;
  const w = boxSize(), last = roll.crops[roll.crops.length - 1];
  const c = last ? Math.min(m.rows, last.c + Math.round((last.w + w) / 2)) : Math.round(w / 2);
  roll.crops.push({ c, w });
  roll.cur = roll.crops.length - 1;
}
function delFrame() {
  if (!roll.crops.length) return;
  roll.crops.splice(roll.cur, 1);
  roll.cur = Math.max(0, Math.min(roll.cur, roll.crops.length - 1));
}
// Changing the format resizes the selected box.
function formatChanged() {
  const cp = roll.crops[roll.cur];
  if (cp) cp.w = boxSize();
  wrap.value?.focus({ preventScroll: true });
}

// A click on the strip starts the current box there (its leading edge, not
// its centre, lands on the click).
function stripClick(e: MouseEvent) {
  if (!inner.value) return;
  const rect = inner.value.getBoundingClientRect();
  if (!roll.crops.length) { roll.crops.push({ c: 0, w: boxSize() }); roll.cur = 0; }
  const cp = roll.crops[roll.cur];
  cp.c = Math.round((e.clientX - rect.left) / pxPerRow() + cp.w / 2);
}

// Auto-frame: the OEM framing in the selected format replaces the boxes.
const autoBusy = ref(false);
async function autoFrame() {
  const m = meta.value;
  if (!m) return;
  autoBusy.value = true;
  roll.framesError = '';
  try {
    const resp = await postJSON('/api/autoframe', { format: format.value });
    const d = await resp.json();
    if (!resp.ok) throw new Error(d.error || 'auto-frame failed');
    m.format = d.format;
    m.pitch = d.pitch;
    roll.crops = d.centres.map((c: number) => ({ c, w: d.frame_w }));
    roll.cur = 0;
    if (!roll.crops.length) roll.framesError = 'No frames found in this format — place them by hand';
  } catch (err) {
    roll.framesError = (err as Error).message;
  }
  autoBusy.value = false;
  wrap.value?.focus({ preventScroll: true });
}

// keyboard: ←/→ nudge (Shift = coarse), ↑/↓ previous/next, Enter adds a
// box, Delete/Backspace removes the current one
function onKey(e: KeyboardEvent) {
  if (!shown.value || e.defaultPrevented) return;
  const t = e.target as HTMLElement | null;
  if (t?.closest?.('select, input, textarea, [role="dialog"]')) return;   // a form control has the keys
  const k = e.key, big = e.shiftKey ? 6 : 1;
  if (k === 'ArrowLeft') nudge(-step() * big);
  else if (k === 'ArrowRight') nudge(step() * big);
  else if (k === 'ArrowUp') go(-1);
  else if (k === 'ArrowDown') go(1);
  else if (k === 'Enter') addFrame();
  else if (k === 'Delete' || k === 'Backspace') delFrame();
  else return;
  e.preventDefault();
}
onMounted(() => { window.addEventListener('keydown', onKey); window.addEventListener('resize', onResize); });
onBeforeUnmount(() => { window.removeEventListener('keydown', onKey); window.removeEventListener('resize', onResize); });

// ── Export the boxes at full resolution ──────────────────────────────────
const exporting = ref(false);
const exp = ref({ show: false, value: 0, label: '' });
async function exportFrames() {
  exporting.value = true;
  setBusy('frames', true);
  exp.value = { show: true, value: 0, label: '' };
  resetExport();
  await postSSE('/api/export_frames', {
    centres: roll.crops.map(c => Math.round(c.c)),
    widths: roll.crops.map(c => Math.round(c.w)),
    ice: ice.value && iceOk.value,
    sharpen: sharpen.value,
  }, ev => {
    if (ev.type === 'progress') exp.value = { show: true, value: ev.pct, label: ev.step };
    else if (ev.type === 'done') {
      exp.value = { show: true, value: 1, label: 'Done' };
      loadFrames().then(() => { exp.value.show = false; showSpace('export'); });
    } else if (ev.type === 'error') exp.value = { ...exp.value, label: ev.message ?? '' };
  });
  exporting.value = false;
  setBusy('frames', false);
}
</script>

<template>
  <!-- nothing to place yet: no roll, or still processing -->
  <Card v-if="!meta" class="px-6 py-12 text-center">
    <h2 class="font-display text-xl font-bold uppercase">{{ roll.wait.title }}</h2>
    <p class="mt-1.5 text-[0.92rem] text-muted">{{ roll.wait.msg }}</p>
    <div class="mx-auto max-w-[420px]"><ProgressBar :show="roll.wait.pct != null" :value="roll.wait.pct" /></div>
  </Card>

  <Card v-else ruled>
    <template #title>{{ title }}</template>
    <template #info>
      <span v-if="strips.length > 1" class="border-2 border-ink px-2 py-0.5 font-mono text-[0.72rem] font-bold uppercase">{{ strips.length }} strips</span>
      <span v-if="meta?.dx_order" class="border-2 border-ink bg-accent px-2 py-0.5 font-mono text-[0.72rem] font-bold uppercase">DX order</span>
      <InfoTip label="Keys" place="below" tip="Click the strip to place a box. ← → nudge · ↑ ↓ previous / next · Enter add · Delete remove." />
    </template>
    <div class="flex flex-wrap items-center justify-between gap-3">
      <div class="flex flex-wrap items-center gap-3">
        <label class="flex items-center gap-2 font-mono text-[0.85rem] font-semibold">Format
          <select v-model="format" aria-label="Picture format" @change="formatChanged">
            <option value="full">Full frame</option>
            <option value="half">Half frame</option>
            <option value="pano">Panoramic</option>
          </select>
        </label>
        <Btn variant="ghost" title="Put a box on every frame" :disabled="autoBusy" @click="autoFrame">Auto-frame</Btn>
        <span class="h-7 w-0.5 bg-ink" aria-hidden="true"></span>
        <Btn variant="ghost" title="Add a box (Enter)" @click="addFrame">+ Add</Btn>
        <Btn variant="ghost" title="Remove the selected box (Delete)" @click="delFrame">− Remove</Btn>
      </div>
      <div class="flex flex-wrap items-center gap-3 text-[0.85rem]">
        <label :class="['inline-flex items-center gap-1.5 whitespace-nowrap', iceOk ? 'cursor-pointer' : 'cursor-not-allowed opacity-45']">
          <input v-model="ice" type="checkbox" :disabled="!iceOk"> Remove dust</label>
        <InfoTip :tip="iceTip" label="About dust removal" place="left" />
        <label class="inline-flex items-center gap-1.5 whitespace-nowrap">Sharpen
          <select v-model="sharpen" aria-label="Sharpening">
            <option value="off">Off</option>
            <option value="low">Low</option>
            <option value="medium">Medium</option>
            <option value="high">High</option>
          </select></label>
        <Btn :disabled="!roll.crops.length || exporting" @click="exportFrames">Export {{ roll.crops.length }} frames</Btn>
      </div>
    </div>
    <div v-if="roll.framesError" class="mt-2.5 font-mono text-[0.83rem] font-semibold text-danger">{{ roll.framesError }}</div>

    <div ref="wrap" tabindex="-1" class="strip relative mt-3.5 overflow-x-auto overflow-y-hidden border-2 border-ink bg-ink py-2.5 outline-none">
      <div ref="inner" class="relative my-2 leading-0" :style="{ width: stripW + 'px' }" @click.self="stripClick">
        <img :src="'/api/preview?t=' + roll.previewStamp" alt="film strip" class="block max-w-none"
             :style="{ width: stripW + 'px', height: stripH + 'px' }" @click="stripClick">
        <!-- everything but the selected box dimmed, to both ends of the strip
             (a spread shadow stops ~10000 px away, short of a long roll) -->
        <template v-if="active">
          <div class="pointer-events-none absolute inset-y-0 left-0 bg-black/65" :style="{ width: Math.max(0, active.left) + 'px' }"></div>
          <div class="pointer-events-none absolute inset-y-0 right-0 bg-black/65" :style="{ left: active.left + active.width + 'px' }"></div>
        </template>
        <div v-for="b in boxes" :key="b.i" @click.stop="select(b.i)"
             :class="['absolute inset-y-0 box-border cursor-pointer',
                      b.i === roll.cur ? 'z-10 border-3 border-danger' : 'border-2 border-accent/80']"
             :style="{ left: b.left + 'px', width: b.width + 'px' }">
          <span :class="['absolute top-0.5 left-1 border border-black px-1 font-mono text-[11px] leading-normal font-semibold',
                         b.i === roll.cur ? 'bg-danger text-white' : 'bg-accent text-black']">{{ b.i + 1 }}</span>
        </div>
        <!-- strip mode: a line where each cut strip starts, its feed number below -->
        <template v-if="stripMarks.length > 1">
          <div v-for="m in stripMarks" :key="'s' + m.i" class="pointer-events-none absolute inset-y-0 z-20" :style="{ left: m.left + 'px' }">
            <div v-if="m.i > 0" class="absolute inset-y-0 -left-0.5 w-1 bg-surface"></div>
            <span class="absolute bottom-0.5 left-1.5 border border-black bg-surface px-1 font-mono text-[11px] leading-normal font-bold whitespace-nowrap text-ink">Strip {{ m.fed }}</span>
          </div>
        </template>
      </div>
    </div>
    <div class="mt-2.5"><ProgressBar :show="exp.show" :value="exp.value" :label="exp.label" class="mt-0!" /></div>
  </Card>
</template>
