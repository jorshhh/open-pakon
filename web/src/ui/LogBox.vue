<script setup lang="ts">
// A job's log lines behind a Log toggle, closed until opened, kept scrolled
// to the latest. Anything the operator must act on is shown outside it.
import { nextTick, ref, watch } from 'vue';

const props = defineProps<{ lines: string[] }>();
const open = ref(false);
const box = ref<HTMLElement | null>(null);
async function toBottom() {
  await nextTick();
  if (box.value) box.value.scrollTop = box.value.scrollHeight;
}
watch(() => props.lines.length, toBottom);
watch(open, v => { if (v) toBottom(); });
</script>

<template>
  <div v-if="lines.length" class="mt-3.5">
    <button type="button" :aria-expanded="open" @click="open = !open"
            class="border-2 border-ink bg-surface px-2.5 py-1 font-mono text-[0.76rem] font-bold hover:bg-accent">
      Log {{ open ? '▾' : '▸' }}
    </button>
    <div v-if="open" ref="box"
         class="mt-2 max-h-30 overflow-y-auto border-2 border-ink bg-ink p-2.5 font-mono text-[0.76rem] whitespace-pre-wrap text-ok shadow-[inset_2px_2px_0_rgba(0,0,0,0.5)]">{{ lines.join('\n') }}</div>
  </div>
</template>
