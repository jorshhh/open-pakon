<script setup lang="ts">
// Crash reports / usage statistics: asked once; Settings changes it.
import { computed, ref } from 'vue';
import { app } from '../state';
import { saveConsent } from '../telemetry';
import Btn from '../ui/Btn.vue';

const t = computed(() => app.status?.telemetry);
const crashes = ref(true);
const usage = ref(true);
</script>

<template>
  <div v-if="t && t.available && !t.asked" role="dialog" aria-labelledby="consent-title"
       class="fixed right-5 bottom-24 z-[60] w-[340px] max-w-[calc(100vw-32px)] border-2 border-ink bg-surface px-4 pt-4 pb-3.5 shadow-hard max-sm:inset-x-4 max-sm:w-auto">
    <h2 id="consent-title" class="mb-1.5 font-display text-base font-bold uppercase">Help improve Pakon</h2>
    <p class="mb-2.5 text-[0.82rem] leading-snug">Send anonymous reports? No photos, nothing personal.</p>
    <div class="mb-2 grid gap-1.5 font-mono text-[0.78rem] font-semibold uppercase">
      <label class="flex cursor-pointer items-center gap-2"><input v-model="crashes" type="checkbox"> Crash reports</label>
      <label class="flex cursor-pointer items-center gap-2"><input v-model="usage" type="checkbox"> Usage statistics</label>
    </div>
    <details class="mb-3 text-[0.76rem] leading-snug text-muted">
      <summary class="cursor-pointer font-semibold text-ink">What's sent?</summary>
      <ul class="mt-1.5 ml-4 list-disc">
        <li>Crash reports: the error, and for a failed scan the scanner's last log lines.</li>
        <li>Usage: pages opened, how long scans, processing and exports take, film type, scan resolution, frame count, whether dust removal was on, the scanner's light calibration.</li>
        <li>Never: photos, file or folder names, your scanner's serial number. Reports carry a random ID.</li>
      </ul>
    </details>
    <div class="flex flex-wrap items-center gap-2">
      <Btn @click="saveConsent(usage, crashes)">Save</Btn>
      <Btn variant="ghost" @click="saveConsent(false, false)">No thanks</Btn>
    </div>
  </div>
</template>
