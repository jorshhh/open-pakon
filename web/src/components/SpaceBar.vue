<script setup lang="ts">
// The space switcher along the bottom, like a Resolve page bar. The body
// keeps room for it: its height and a gap.
import { onBeforeUnmount, onMounted, ref } from 'vue';
import { app, showSpace, type Space } from '../state/app';

const tabs: { id: Space; name: string; icon: string }[] = [
  { id: 'home', name: 'Home', icon: '<path d="M3 11l9-7 9 7"/><path d="M5 9.5V20h14V9.5"/><path d="M10 20v-6h4v6"/>' },
  { id: 'scan', name: 'Scan', icon: '<rect x="3" y="6" width="18" height="12"/><path d="M3 9h18M3 15h18M7 6v3M11 6v3M15 6v3M19 6v3M7 15v3M11 15v3M15 15v3M19 15v3"/>' },
  { id: 'process', name: 'Process', icon: '<rect x="3" y="5" width="18" height="14"/><rect x="6" y="8" width="7" height="8"/><path d="M16 9h2M16 12h2M16 15h2"/>' },
  { id: 'frames', name: 'Frames', icon: '<rect x="2" y="7" width="9" height="10"/><rect x="13" y="7" width="9" height="10"/><path d="M2 4h20M2 20h20"/>' },
  { id: 'adjust', name: 'Adjust', icon: '<path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0"/><rect x="14" y="4" width="4" height="4"/><rect x="8" y="10" width="4" height="4"/><rect x="16" y="16" width="4" height="4"/>' },
  { id: 'export', name: 'Export', icon: '<path d="M12 3v12M7 10l5 5 5-5"/><path d="M4 15v5h16v-5"/>' },
  { id: 'settings', name: 'Settings', icon: '<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M2 12h3M19 12h3M4.9 19.1L7 17M17 7l2.1-2.1"/>' },
];

const bar = ref<HTMLElement | null>(null);
const fit = () => { if (bar.value) document.body.style.paddingBottom = (bar.value.offsetHeight + 16) + 'px'; };
let ro: ResizeObserver | null = null;
onMounted(() => { fit(); ro = new ResizeObserver(fit); if (bar.value) ro.observe(bar.value); });
onBeforeUnmount(() => ro?.disconnect());
</script>

<template>
  <nav ref="bar" class="fixed inset-x-0 bottom-0 z-50 flex justify-center gap-1 bg-ink px-4 pt-1.5 pb-2" role="tablist" aria-label="Workspace">
    <button v-for="t in tabs" :key="t.id" role="tab" :id="'tab-' + t.id" :aria-controls="'space-' + t.id"
            :aria-selected="app.space === t.id" @click="showSpace(t.id)"
            :class="['relative flex min-w-24 cursor-pointer flex-col items-center gap-1 border-t-3 px-3.5 py-2 font-mono text-small font-semibold tracking-[0.06em] uppercase focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
                     app.space === t.id ? 'border-accent text-accent' : 'border-transparent text-tab hover:text-surface']">
      <svg class="size-6" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" aria-hidden="true" v-html="t.icon"></svg>{{ t.name }}
      <!-- work still running in that space -->
      <span v-if="app.busy[t.id]" class="absolute top-2 right-6.5 size-2 animate-blink rounded-full bg-danger"></span>
    </button>
  </nav>
</template>
