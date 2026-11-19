<script setup lang="ts">
// The keys a space answers to, always on show: each key a small cap, then
// what it does.
defineProps<{ hints: { keys: string[]; what: string }[] }>();
</script>

<template>
  <p class="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-small text-muted">
    <span v-for="h in hints" :key="h.what" class="inline-flex items-center gap-1 whitespace-nowrap">
      <kbd v-for="k in h.keys" :key="k" class="min-w-5 border border-ink bg-surface px-1 text-center font-mono text-small font-semibold text-ink shadow-[1px_1px_0_0_var(--color-ink)]">{{ k }}</kbd>
      <span class="ml-0.5">{{ h.what }}</span>
    </span>
  </p>
</template>
