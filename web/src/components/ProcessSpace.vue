<script setup lang="ts">
// The open raw, and processing it (the prescan: a long preview + detected
// frame centres), which opens the roll in Frames.
import { computed } from 'vue';
import { splitPath } from '../state/app';
import { FILMS, pick, processRaw, roll } from '../state/roll';
import Btn from '../ui/Btn.vue';
import Card from '../ui/Card.vue';
import InfoTip from '../ui/InfoTip.vue';
import MsgLine from '../ui/MsgLine.vue';
import ProgressBar from '../ui/ProgressBar.vue';

// file name on top, its folder underneath
const name = computed(() => (roll.path ? splitPath(roll.path).name : 'No raw open'));
const dir = computed(() => (roll.path ? splitPath(roll.path).dir : ''));
const dx = computed(() => roll.dx ? `DX: ${roll.dx.name ? roll.dx.name + ' · ' : ''}ISO ${roll.dx.iso}` : '');

// Picking another film type processes the open raw again.
function filmChanged() { if (roll.path) processRaw(); }
</script>

<template>
  <Card title="Process">
    <template #info>
      <InfoTip label="About opening a raw"
               tip="A .raw is read where it is, never copied. Its frames open in Frames, with any boxes you saved before." />
    </template>
    <div :class="['flex flex-wrap items-center gap-3.5 border-2 border-ink px-3.5 py-3', roll.path ? 'bg-surface-2' : 'border-dashed']" :title="roll.path">
      <div class="min-w-0 flex-[1_1_220px] font-mono">
        <div :class="['truncate text-body', roll.path ? 'font-semibold' : 'text-muted']">{{ name }}</div>
        <div v-if="dir" class="mt-0.5 truncate text-small text-muted">{{ dir }}</div>
        <div v-if="dx" class="mt-0.5 truncate text-small text-muted">{{ dx }}</div>
      </div>
      <label class="flex items-center gap-2 font-mono text-body font-semibold">Film
        <select v-model="roll.film" aria-label="Film type of the raw" :disabled="roll.processing" @change="filmChanged">
          <option v-for="f in FILMS" :key="f.id" :value="f.id">{{ f.name }}</option>
        </select>
      </label>
      <Btn :disabled="roll.processing" @click="pick(true)">Open…</Btn>
    </div>
    <ProgressBar :show="roll.processing" :value="roll.pct" :label="roll.step" />
    <MsgLine v-if="roll.procMsg.text" :msg="roll.procMsg" tag="div" class="mt-2" />
  </Card>
</template>
