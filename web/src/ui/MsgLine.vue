<script setup lang="ts">
// A short status line: grey, green (ok) or red (err).
import type { Msg } from '../state';
defineProps<{ msg: Msg; tag?: string }>();
</script>

<template>
  <component :is="tag || 'span'" :title="msg.title"
             :class="['font-mono text-[0.83rem] font-semibold', msg.kind === 'ok' ? 'text-ok' : msg.kind === 'err' ? 'text-danger' : 'text-muted']">{{ msg.text }}</component>
</template>
