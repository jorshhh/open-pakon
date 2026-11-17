<script setup lang="ts">
// Export: the exported frames, and saving them into the work folder.
import { computed, ref, watch } from 'vue';
import { postJSON, postSSE } from '../api';
import { exported, msg, setBusy } from '../state';
import Btn from '../ui/Btn.vue';
import Card from '../ui/Card.vue';
import EmptyCard from '../ui/EmptyCard.vue';
import MsgLine from '../ui/MsgLine.vue';
import ProgressBar from '../ui/ProgressBar.vue';

const n = computed(() => exported.frames.length);
const formats = [
  { fmt: 'jpeg', name: 'JPEGs' },
  { fmt: 'tiff', name: 'TIFFs' },
  { fmt: 'raw', name: 'Raw negatives' },
  { fmt: 'contact', name: 'Contact sheet' },
];

const saving = ref(false);
const prog = ref({ show: false, value: 0, label: '' });
const result = ref(msg());
const savedPath = ref('');      // what Show in Finder / Open folder opens

// a new export starts clean: no message or folder from the last one
watch(() => exported.stamp, () => {
  result.value = msg();
  savedPath.value = '';
  prog.value = { show: false, value: 0, label: '' };
});

async function save(fmt: string) {
  saving.value = true;
  setBusy('export', true);
  result.value = msg();
  savedPath.value = '';
  prog.value = { show: true, value: 0, label: 'Starting…' };
  await postSSE('/api/save', { fmt }, ev => {
    if (ev.type === 'progress') prog.value = { show: true, value: ev.pct, label: ev.step };
    else if (ev.type === 'done') {
      prog.value.show = false;        // the message says it
      savedPath.value = ev.file || ev.folder || '';
      result.value = msg(ev.count === 1 && ev.file ? `Saved the ${ev.what}` : `Saved ${ev.count} ${ev.what}`, 'ok', savedPath.value);
    } else if (ev.type === 'error') {
      prog.value.show = false;
      result.value = msg(ev.message, 'err');
    }
  });
  saving.value = false;
  setBusy('export', false);
}
</script>

<template>
  <EmptyCard v-if="!n" title="Nothing exported yet">Export from <b>Frames</b>.</EmptyCard>
  <template v-else>
    <Card :title="`Save ${n} frame${n === 1 ? '' : 's'}`" ruled>
      <div class="mb-3.5 font-mono text-[0.88rem] font-semibold" :title="exported.folder">{{ exported.folder || 'the work folder' }}</div>
      <div class="grid grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-3">
        <Btn v-for="f in formats" :key="f.fmt" variant="ghost" :disabled="saving"
             class="bg-surface-2! px-4! py-3.5! text-left normal-case enabled:hover:bg-accent!" @click="save(f.fmt)">
          <span class="font-display text-base font-bold uppercase">{{ f.name }}</span>
        </Btn>
      </div>
      <ProgressBar :show="prog.show" :value="prog.value" :label="prog.label" />
      <div class="mt-3 flex flex-wrap items-center gap-3">
        <MsgLine :msg="result" />
        <!-- only where the service can open a folder here (Mac, Linux desktop) -->
        <Btn v-if="savedPath && exported.reveal" variant="ghost" @click="postJSON('/api/reveal', { path: savedPath })">{{ exported.reveal }}</Btn>
      </div>
    </Card>

    <Card :title="`Frames (${n})`" ruled class="mt-1">
      <div class="grid grid-cols-[repeat(auto-fill,minmax(190px,1fr))] gap-4">
        <div v-for="f in exported.frames" :key="f.index" class="overflow-hidden border-2 border-ink bg-surface shadow-hard-sm">
          <img :src="`/api/frames/${f.index}/thumb?t=${exported.stamp}`" :alt="`Frame ${f.index}`" loading="lazy"
               class="block h-32.5 w-full border-b-2 border-ink bg-ink object-cover">
          <div class="bg-paper px-2.5 py-2 font-mono text-[0.75rem] font-semibold">Frame {{ f.index }}</div>
        </div>
      </div>
    </Card>
  </template>
</template>
