<script setup lang="ts">
// The scanner's state as a dot: green ready, orange needs firmware, grey else.
defineProps<{ state: string }>();
</script>

<template>
  <span :class="['size-2.5 shrink-0 rounded-full border border-ink transition-colors duration-300',
                 state === 'warm' ? 'bg-ok' : state === 'cold' ? 'bg-warn' : 'bg-muted']"></span>
</template>
