<script setup lang="ts">
// A cold scanner: load the firmware (RAM only; a power cycle needs it again).
import { ref } from 'vue';
import { postSSE } from '../api';
import { msg } from '../state/app';
import Btn from '../ui/Btn.vue';
import Card from '../ui/Card.vue';
import LogBox from '../ui/LogBox.vue';
import MsgLine from '../ui/MsgLine.vue';
import ProgressBar from '../ui/ProgressBar.vue';

const running = ref(false);
const done = ref(false);
const started = ref(false);
const status = ref(msg());
const lines = ref<string[]>([]);

async function load() {
  running.value = started.value = true;
  done.value = false;
  status.value = msg();
  lines.value = [];
  await postSSE('/api/firmware', {}, ev => {
    if (ev.type === 'log') lines.value.push(ev.message ?? '');
    else if (ev.type === 'done') { done.value = true; status.value = msg('Firmware loaded', 'ok'); }
    else if (ev.type === 'error') status.value = msg(ev.message, 'err');
  });
  running.value = false;
}
</script>

<template>
  <Card title="Firmware">
    <div class="flex flex-wrap items-center gap-3">
      <Btn :disabled="running" @click="load">Load firmware</Btn>
      <MsgLine :msg="status" />
    </div>
    <ProgressBar :show="started" :value="done ? 1 : null" />
    <LogBox :lines="lines" />
  </Card>
</template>
