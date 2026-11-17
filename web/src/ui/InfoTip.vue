<script setup lang="ts">
// The round "i": the longer explanation, on hover or keyboard focus.
defineProps<{ tip: string; label: string; place?: 'left' | 'below' }>();
</script>

<template>
  <button type="button" :data-tip="tip" :aria-label="label"
          :class="['tip relative inline-grid size-5 shrink-0 cursor-help place-items-center rounded-full border-2 border-ink bg-surface p-0 align-middle font-mono text-[0.72rem] leading-none font-bold',
                   'hover:bg-accent focus-visible:bg-accent focus-visible:outline-none', place && 'tip-' + place]">i</button>
</template>
