<script setup lang="ts">
// The one-line text dialog (ask.ts): just the input, Cancel and the action.
import { nextTick, ref, watch } from 'vue';
import { asking, finishAsk } from '../ask';
import Btn from '../ui/Btn.vue';
import Modal from '../ui/Modal.vue';

const input = ref<HTMLInputElement | null>(null);
const error = ref('');

watch(() => asking.open, async open => {
  error.value = '';
  if (!open) return;
  await nextTick();
  input.value?.focus();
  input.value?.select();
});

function submit() {
  const v = asking.value.trim();
  const err = asking.check ? asking.check(v) : '';
  if (err) { error.value = err; return; }
  finishAsk(v);
}

// capture, so Esc closes this dialog and not the folder browser under it
window.addEventListener('keydown', e => {
  if (e.key === 'Escape' && asking.open) { e.stopImmediatePropagation(); finishAsk(null); }
}, true);
</script>

<template>
  <Modal v-if="asking.open" as="form" small top :aria-label="asking.label" @close="finishAsk(null)" @submit.prevent="submit">
    <div class="px-5 py-4.5">
      <input ref="input" v-model="asking.value" type="text" maxlength="64" autocomplete="off" spellcheck="false"
             :aria-label="asking.label" class="w-full font-display! text-heading! font-bold" @input="error = ''">
      <p v-if="error" class="mt-2 font-mono text-small font-semibold text-danger">{{ error }}</p>
    </div>
    <template #foot>
      <span class="flex-1"></span>
      <Btn type="button" variant="ghost" @click="finishAsk(null)">Cancel</Btn>
      <Btn type="submit">{{ asking.ok }}</Btn>
    </template>
  </Modal>
</template>
