<script setup lang="ts">
import { showSpace } from '../state/app';
import { pick } from '../state/roll';
import { homeState, homeText } from '../status';
import Card from '../ui/Card.vue';
import Dot from '../ui/Dot.vue';

const choice = 'flex cursor-pointer flex-col items-start gap-2 border-2 border-ink px-5 py-5.5 text-left shadow-hard transition-all duration-150 hover:-translate-0.5 active:translate-0.5 active:shadow-none [&_svg]:mb-1 [&_svg]:size-9';
const name = 'font-display text-title font-bold uppercase';
</script>

<template>
  <Card class="px-8 py-10">
    <h2 class="mb-6 font-display text-title font-bold tracking-tight uppercase">Welcome</h2>
    <div class="grid grid-cols-[repeat(auto-fit,minmax(240px,1fr))] gap-4">
      <button :class="[choice, 'bg-accent']" @click="showSpace('scan')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" aria-hidden="true"><rect x="3" y="6" width="18" height="12"/><path d="M3 9h18M3 15h18M7 6v3M11 6v3M15 6v3M19 6v3M7 15v3M11 15v3M15 15v3M19 15v3"/></svg>
        <span :class="name">Start a scan</span>
        <span class="mt-1 flex items-center gap-1.5 font-mono text-small font-semibold uppercase"><Dot :state="homeState" />{{ homeText }}</span>
      </button>
      <button :class="[choice, 'bg-surface-2']" @click="pick(true)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" aria-hidden="true"><path d="M3 6h7l2 2h9v11H3z"/><path d="M12 11v5M9.5 13.5L12 16l2.5-2.5"/></svg>
        <span :class="name">Open a raw</span>
      </button>
    </div>
  </Card>
</template>
