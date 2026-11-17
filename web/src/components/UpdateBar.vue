<script setup lang="ts">
// A newer release is out (pakon-web update.rs). Shown until Later; it
// comes back at the next launch.
import { computed, ref, watch } from 'vue';
import { app } from '../state';
import Btn from '../ui/Btn.vue';

const u = computed(() => app.status?.update ?? null);
const busyMsg = ref('');
// the normal line comes back at the next status poll, as it always did
watch(() => app.status, () => { busyMsg.value = ''; });

const text = computed(() => {
  if (busyMsg.value) return busyMsg.value;
  if (!u.value) return '';
  // install: the Mac app updates itself (Sparkle) and restarts
  return u.value.install
    ? `You have ${u.value.current}. Pakon restarts to update.`
    : `You have ${u.value.current}. Download the new version, quit Pakon, ` +
      'and drag the new one into Applications to replace it.';
});

async function get() {
  const d = await fetch('/api/update/open', { method: 'POST' }).then(r => r.json());
  if (d.busy) { busyMsg.value = 'Finish the scan first.'; return; }
  if (d.installing) return;
  if (!d.opened) window.open(d.url, '_blank', 'noopener');
}
</script>

<template>
  <div v-if="u && !app.updateLater" role="status"
       class="-mt-4 mb-6 flex flex-wrap items-center gap-3.5 border-2 border-ink bg-accent px-4 py-3 shadow-hard">
    <svg class="size-6.5 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" aria-hidden="true"><path d="M12 3v12M7 10l5 5 5-5"/><path d="M4 15v5h16v-5"/></svg>
    <div class="min-w-0 flex-[1_1_260px] text-[0.9rem] leading-snug">
      <strong class="block font-display text-[1.05rem] uppercase">Pakon {{ u.latest }} is out</strong>
      <span>{{ text }}</span>
    </div>
    <Btn variant="dark" @click="get">{{ u.install ? 'Install' : 'Download' }}</Btn>
    <Btn variant="ghost" @click="app.updateLater = true">Later</Btn>
  </div>
</template>
