<script setup lang="ts">
import { app } from '../state';
import { scannerState } from '../status';
import EmptyCard from '../ui/EmptyCard.vue';
import FirmwareCard from './FirmwareCard.vue';
import ScanCard from './ScanCard.vue';
</script>

<template>
  <FirmwareCard v-show="scannerState === 'cold'" />
  <ScanCard v-show="scannerState === 'warm'" />
  <EmptyCard v-if="scannerState && scannerState !== 'cold' && scannerState !== 'warm'" title="No scanner">
    {{ app.status?.state === 'unknown' && app.status.state_error ? app.status.state_error : 'Connect it by USB and turn it on.' }}
  </EmptyCard>
</template>
