<script setup lang="ts">
// A space with nothing to show yet.
import Card from './Card.vue';
defineProps<{ title: string }>();
</script>

<template>
  <Card class="px-6 py-12 text-center">
    <h2 class="font-display text-title font-bold uppercase">{{ title }}</h2>
    <p class="mt-1.5 text-body text-muted"><slot /></p>
  </Card>
</template>
