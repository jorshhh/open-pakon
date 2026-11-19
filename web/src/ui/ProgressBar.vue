<script setup lang="ts">
// A progress bar with a line under it; `value` null = indeterminate.
defineProps<{ show: boolean; value?: number | null; max?: number; label?: string }>();
</script>

<template>
  <div v-if="show" class="mt-4 min-w-0">
    <progress v-if="value == null"></progress>
    <progress v-else :value="value" :max="max ?? 1"></progress>
    <div v-if="label" class="mt-1.5 truncate font-mono text-small text-muted">{{ label }}</div>
  </div>
</template>
