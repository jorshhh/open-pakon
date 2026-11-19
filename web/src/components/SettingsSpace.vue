<script setup lang="ts">
// Settings: the reports, troubleshooting, about.
import { computed, ref } from 'vue';
import { app, msg } from '../state/app';
import { saveConsent } from '../telemetry';
import Btn from '../ui/Btn.vue';
import Card from '../ui/Card.vue';
import InfoTip from '../ui/InfoTip.vue';
import MsgLine from '../ui/MsgLine.vue';

const t = computed(() => app.status?.telemetry);
const on = computed(() => !!t.value?.available);
const saved = ref(msg());

async function setReport(key: 'usage' | 'crashes', value: boolean) {
  const cur = t.value;
  if (!cur) return;
  const next = { usage: cur.usage, crashes: cur.crashes, [key]: value };
  await saveConsent(next.usage, next.crashes);
  saved.value = msg('Saved', 'ok');
}

// Bug report by email: opened by the service (the app window cannot follow
// a mailto: link); in a browser, the page opens the link itself.
const reportMsg = ref(msg());
async function reportProblem() {
  reportMsg.value = msg();
  try {
    const d = await fetch('/api/report', { method: 'POST' }).then(r => r.json());
    if (!d.opened) window.location.href = d.url;
    reportMsg.value = msg('Opening your mail app… attach pakon.log from Show log.', 'ok');
  } catch {
    reportMsg.value = msg('Could not open the mail app. Write to jorsh@jor.sh.', 'err');
  }
}

const openLogs = () => fetch('/api/logs/open', { method: 'POST' });

const reports = [
  { key: 'crashes' as const, name: 'Crash reports', tip: 'When something fails: the error, and for a failed scan the scanner\'s last log lines.' },
  { key: 'usage' as const, name: 'Usage statistics', tip: 'Pages opened, and how long scans, processing and exports take, with the film type and frame count, and the scanner\'s light calibration.' },
];
const note = 'mb-2 max-w-[72ch] text-small leading-snug text-muted';
</script>

<template>
  <Card title="Privacy">
    <template #info>
      <InfoTip label="About reports"
               tip="Anonymous reports show what breaks on other scanners. Never sent: photos, file or folder names, the scanner's serial. Turning both off forgets the random ID." />
    </template>
    <div class="mb-3.5 grid max-w-[480px] gap-2.5">
      <div v-for="r in reports" :key="r.key" class="flex items-center gap-2.5">
        <label :class="['flex flex-1 items-center gap-3 border-2 border-ink bg-surface-2 px-3.5 py-3', on ? 'cursor-pointer' : 'cursor-not-allowed opacity-50']">
          <input type="checkbox" :disabled="!on" :checked="on && !!t?.[r.key]"
                 @change="setReport(r.key, ($event.target as HTMLInputElement).checked)">
          <strong class="font-mono text-small uppercase">{{ r.name }}</strong>
        </label>
        <InfoTip :label="'About ' + r.name.toLowerCase()" :tip="r.tip" />
      </div>
    </div>
    <p v-if="on" :class="note">Sent to a self-hosted
      <a class="underline" href="https://rybbit.com" target="_blank" rel="noopener">Rybbit</a>, not a third party.</p>
    <p v-else :class="note">This build sends no reports.</p>
    <MsgLine :msg="saved" />
  </Card>

  <Card title="Troubleshooting">
    <dl class="mb-4.5 grid max-w-[760px] grid-cols-1 items-baseline gap-x-4.5 gap-y-0.5 sm:grid-cols-[max-content_1fr] sm:gap-y-2.5 [&_dd]:text-body [&_dd]:leading-snug max-sm:[&_dd]:mb-2.5 [&_dt]:font-mono [&_dt]:text-small [&_dt]:font-semibold [&_dt]:uppercase">
      <dt>Calibration fails</dt>
      <dd>Switch the scanner off and on, then open Pakon before TLX or PakonScan.
        <InfoTip class="ml-1.5" label="Why"
                 tip="Other scanning software leaves the scanner in its own state until it is switched off. Keep the film out until the motor starts." /></dd>
      <dt>Scanner not found</dt>
      <dd>Check the USB cable and power, then switch the scanner off and on.
        <InfoTip class="ml-1.5" label="On Windows"
                 tip="On Windows, run pakon_driver.exe in %LOCALAPPDATA%\Programs\Pakon\driver to set up the scanner's driver again." /></dd>
      <dt>Film stuck</dt>
      <dd><b>Advance film</b> on Scan.</dd>
    </dl>
    <div class="flex flex-wrap items-center gap-3">
      <Btn @click="reportProblem">Report a problem</Btn>
      <!-- only where the service can open the log folder -->
      <Btn v-if="app.status?.logs" variant="ghost" @click="openLogs">Show log</Btn>
      <MsgLine :msg="reportMsg" />
    </div>
  </Card>

  <Card title="About">
    <p :class="note">Pakon {{ app.status?.version || '(from source)' }}. Not affiliated with Kodak or Pakon.</p>
  </Card>
</template>
