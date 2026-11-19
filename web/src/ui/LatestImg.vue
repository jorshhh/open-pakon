<script setup lang="ts">
// An image that only loads the newest `src` asked for and keeps showing the
// last one until it is ready: quick clicks skip the renders in between and
// the picture never blanks.
import { ref, watch } from 'vue';

const props = defineProps<{ src: string; alt?: string }>();
const shown = ref('');
let loading = '';
let want = '';

function next() {
  if (loading || want === shown.value) return;
  const s = (loading = want);
  const img = new Image();
  img.src = s;
  img.decode()
    .then(() => { shown.value = s; })
    .catch(() => {})
    .finally(() => { loading = ''; next(); });
}
watch(() => props.src, s => { want = s; next(); }, { immediate: true });
</script>

<template>
  <img v-if="shown" :src="shown" :alt="alt ?? ''" draggable="false">
</template>
