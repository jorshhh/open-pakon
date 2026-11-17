<script setup lang="ts">
// A job's log lines, kept scrolled to the latest.
import { nextTick, ref, watch } from 'vue';

const props = defineProps<{ lines: string[] }>();
const box = ref<HTMLElement | null>(null);
watch(() => props.lines.length, async () => {
  await nextTick();
  if (box.value) box.value.scrollTop = box.value.scrollHeight;
});
</script>

<template>
  <div v-if="lines.length" ref="box"
       class="mt-3.5 max-h-30 overflow-y-auto border-2 border-ink bg-ink p-2.5 font-mono text-[0.76rem] whitespace-pre-wrap text-ok shadow-[inset_2px_2px_0_rgba(0,0,0,0.5)]">{{ lines.join('\n') }}</div>
</template>
