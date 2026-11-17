<script setup lang="ts">
// A warm scanner: the work folder, the order, film and resolution; scan,
// stop, advance film.
import { computed, ref, watch } from 'vue';
import { postSSE, store } from '../api';
import { ask } from '../ask';
import { app, FILMS, msg, order, pick, processRaw, remembered, setBusy, showRaw, showSpace, type Film } from '../state';
import Btn from '../ui/Btn.vue';
import Card from '../ui/Card.vue';
import InfoTip from '../ui/InfoTip.vue';
import LogBox from '../ui/LogBox.vue';
import MsgLine from '../ui/MsgLine.vue';
import ProgressBar from '../ui/ProgressBar.vue';

const BASES = ['16', '8', '4'] as const;

// The film type and resolution are remembered in this browser.
const film = ref<Film>(remembered('pakon-scan-film', 'c41', FILMS.map(f => f.id)));
const base = ref(remembered('pakon-scan-base', '16', BASES));
watch(film, v => store.set('pakon-scan-film', v));
watch(base, v => store.set('pakon-scan-base', v));

const st = computed(() => app.status);
const next = computed(() => st.value?.next_order ?? '');

// Whether the scan records the infrared channel (silver B&W does not).
const irOn = computed(() => film.value !== 'bw');
const irTip = computed(() => !irOn.value
  ? 'Silver B&W blocks infrared: no dust removal for this roll.'
  : film.value === 'slide'
  ? 'Infrared is recorded, but dust removal stays off for slides: it softens their grain.'
  : 'Infrared is recorded: dust removal (ICE) can be used at export.');

// Only the exception is worth a word: no calibration data, so the scan is
// a replay.
const replayTitle = computed(() => st.value?.eeprom_unit?.error
  ? `EEPROM: ${st.value.eeprom_unit.error}`
  : 'The EEPROM is read once the scanner is warm; until then scans are replayed');

const scanning = ref(false);
const ejecting = ref(false);
const stopping = ref(false);
const lines = ref<string[]>([]);
const feed = ref('');
const prog = ref({ show: false, value: 0, max: 2200, label: '' });
const log = (s: string) => lines.value.push(s);

// ── The order number (#95): the next number, or one the operator sets ──
// The same rules as the service (orders::custom_order), so the dialog can
// say what is wrong before a scan starts.
function orderProblem(s: string): string {
  if (!s) return '';
  if (/[^\p{L}\p{N} ._-]/u.test(s)) return 'Use letters, digits, spaces, - _ .';
  if (s.startsWith('.') || s.endsWith('.')) return "Can't start or end with “.”";
  const stem = s.split('.')[0].trimEnd().toUpperCase();
  if (/^(CON|PRN|AUX|NUL|COM\d|LPT\d)$/.test(stem)) return 'Not allowed as a folder name on Windows';
  return '';
}
async function editOrder() {
  const v = await ask({ value: order.custom || next.value, label: 'Order number', check: orderProblem });
  if (v === null) return;          // cancelled: keep what was there
  order.custom = v === next.value ? '' : v;
}

async function scan() {
  scanning.value = true;
  stopping.value = false;
  setBusy('scan', true);
  app.scanMsg = msg('Starting…');
  lines.value = [];
  feed.value = '';
  // A roll's raw shrinks with the pixel count: 56% at Base 8, 25% at Base 4,
  // and by a quarter for silver B&W, which scans without the IR channel.
  const b = Number(base.value) as 16 | 8 | 4;
  prog.value = { show: true, value: 0, label: '',
    max: Math.round(2200 * { 16: 1, 8: 0.5625, 4: 0.25 }[b] * (film.value === 'bw' ? 0.75 : 1)) };
  const wanted = order.custom;
  const scanFilm = film.value;
  await postSSE('/api/scan', { film: scanFilm, base: b, order: wanted }, ev => {
    if (ev.type === 'order') {
      log(`Order ${ev.order}: ${ev.path}`);
      order.custom = '';
      if (wanted && ev.order !== wanted) log(`Order ${wanted} already has a folder; this scan is ${ev.order}`);
    } else if (ev.type === 'log') {
      log(ev.message ?? '');
    } else if (ev.type === 'feed') {
      feed.value = ev.message ?? '';
      log(feed.value);
    } else if (ev.type === 'progress') {
      prog.value.value = ev.mb;
      prog.value.label = `${ev.mb} MB`;
      app.scanMsg = msg();
    } else if (ev.type === 'done') {
      prog.value.show = false;       // the message says it
      app.scanMsg = msg(`Done · ${ev.mb} MB`, 'ok');
      if (ev.path) {
        showRaw(ev.path, ev.dx, ev.film || scanFilm);
        showSpace('frames');
        processRaw();
      }
    } else if (ev.type === 'error') {
      app.scanMsg = msg(ev.message, 'err');
    }
  });
  feed.value = '';
  scanning.value = false;
  setBusy('scan', false);
}

// End the roll now (for a strip the end-of-roll check misses).
async function stop() {
  stopping.value = true;
  log('Stop pressed: ending the roll');
  try {
    const r = await fetch('/api/scan/stop', { method: 'POST' });
    if (!r.ok) {
      const d = await r.json().catch(() => ({}));
      log(`Stop: ${d.error || r.status}`);
    }
  } catch (e) {
    log(`Stop failed: ${e}`);
  }
}

// Run the film out of the transport.
async function advance() {
  ejecting.value = true;
  app.scanMsg = msg('Advancing film…');
  lines.value = [];
  await postSSE('/api/eject', {}, ev => {
    if (ev.type === 'log') {
      log(ev.message ?? '');
      if (ev.message?.startsWith('eject:')) app.scanMsg = msg(ev.message.slice(7), 'ok');
    } else if (ev.type === 'done') {
      if (app.scanMsg.text === 'Advancing film…') app.scanMsg = msg('Transport stopped', 'ok');
    } else if (ev.type === 'error') {
      app.scanMsg = msg(ev.message, 'err');
    }
  });
  ejecting.value = false;
}

const pencil = '<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M13 7l4 4"/>';

// the settings table: four, two or one across, never a row with empty cells
const label = 'bg-ink px-3 py-2 font-mono text-[0.72rem] font-semibold tracking-[0.04em] text-surface uppercase';
const body = 'flex min-h-14.5 min-w-0 items-center gap-2.5 px-3 py-2.5 font-mono text-[0.88rem]';
</script>

<template>
  <Card title="Scan">
    <div class="grid grid-cols-1 gap-0.5 border-2 border-ink bg-ink min-[561px]:grid-cols-2 min-[1061px]:grid-cols-4 [&>div]:flex [&>div]:min-w-0 [&>div]:flex-col [&>div]:bg-surface-2">
      <div>
        <span :class="label">Work folder</span>
        <div :class="body">
          <span class="min-w-0 flex-1 truncate font-semibold" :title="st?.work_dir">{{ st?.work_dir || '…' }}</span>
          <Btn variant="ghost" icon title="Change the work folder" aria-label="Change the work folder" @click="pick(false)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="square" aria-hidden="true" v-html="pencil"></svg>
          </Btn>
        </div>
      </div>
      <div>
        <span :class="label">Order</span>
        <div :class="body">
          <span class="min-w-0 flex-1 truncate font-display text-lg font-bold"
                :title="order.custom ? 'Set for the next scan' : 'The next number'">{{ order.custom || next || '—' }}</span>
          <Btn variant="ghost" icon title="Set the order number" aria-label="Set the order number" :disabled="scanning" @click="editOrder">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="square" aria-hidden="true" v-html="pencil"></svg>
          </Btn>
        </div>
      </div>
      <div>
        <label :class="label" for="scan-film">Film type</label>
        <div :class="body">
          <select id="scan-film" v-model="film" class="w-full">
            <option v-for="f in FILMS" :key="f.id" :value="f.id">{{ f.name }}</option>
          </select>
        </div>
      </div>
      <div>
        <label :class="label" for="scan-base">Resolution</label>
        <div :class="body">
          <select id="scan-base" v-model="base" class="w-full">
            <option value="16">Base 16 · 2000 × 3000</option>
            <option value="8">Base 8 · 1500 × 2250</option>
            <option value="4">Base 4 · 1000 × 1500</option>
          </select>
        </div>
      </div>
    </div>
    <div class="mt-4 flex flex-wrap items-center gap-2.5">
      <Btn class="px-7! py-2.5!" :disabled="scanning || ejecting" @click="scan">Scan</Btn>
      <!-- while the scan runs, Stop takes Advance film's place -->
      <Btn v-if="!scanning" variant="ghost" :disabled="ejecting" @click="advance">Advance film</Btn>
      <Btn v-else variant="stop" title="Stop and keep what was scanned" :disabled="stopping" @click="stop">
        {{ stopping ? 'Stopping…' : 'Stop' }}
      </Btn>
      <MsgLine :msg="app.scanMsg" />
      <span class="flex-1"></span>
      <MsgLine v-if="!st?.eeprom" :msg="{ text: 'Replayed scan', title: replayTitle }" />
      <span :class="['border-2 border-ink px-2 py-0.5 font-mono text-[0.78rem] font-bold uppercase', irOn ? 'bg-accent' : 'bg-surface']">{{ irOn ? 'IR on' : 'IR off' }}</span>
      <InfoTip :tip="irTip" label="About infrared" place="left" />
    </div>
    <!-- a warm scanner Pakon did not start -->
    <div v-if="st?.foreign_firmware === true"
         class="mt-3.5 flex items-center gap-2.5 border-2 border-warn bg-[#fdf0e6] px-3 py-2.5 font-mono text-[0.85rem] font-bold text-[#9a3b07]">
      <span class="flex-1">Started by other software. Switch the scanner off and on before scanning.</span>
      <InfoTip label="Why" place="left"
               tip="Pakon didn't load this scanner's firmware, so it may still hold another program's settings (TLX, PakonScan), which can make calibration fail. Switching it off clears them." />
    </div>
    <div v-if="feed" class="mt-3.5 border-2 border-ok p-3 font-mono font-bold text-ok">{{ feed }}</div>
    <ProgressBar :show="prog.show" :value="prog.value" :max="prog.max" :label="prog.label" />
    <LogBox :lines="lines" />
  </Card>
</template>
