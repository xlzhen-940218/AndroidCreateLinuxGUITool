<script setup lang="ts">
export interface DialogButton {
  id: string;
  label: string;
  primary?: boolean;
  danger?: boolean;
}

defineProps<{
  title: string;
  message?: string;
  detail?: string;
  tone?: "info" | "warn" | "error" | "success";
  buttons: DialogButton[];
}>();

const emit = defineEmits<{ (e: "action", id: string): void }>();
</script>

<template>
  <div class="modal-backdrop" @click.self="emit('action', 'dismiss')">
    <div class="modal" :class="`tone-${tone ?? 'info'}`">
      <div class="modal-icon" aria-hidden="true">
        <template v-if="tone === 'error'">!</template>
        <template v-else-if="tone === 'warn'">!</template>
        <template v-else-if="tone === 'success'">✓</template>
        <template v-else>i</template>
      </div>
      <h3 class="modal-title">{{ title }}</h3>
      <p v-if="message" class="modal-message">{{ message }}</p>
      <pre v-if="detail" class="modal-detail">{{ detail }}</pre>
      <div class="modal-actions">
        <button
          v-for="b in buttons"
          :key="b.id"
          class="btn"
          :class="{ primary: b.primary, danger: b.danger }"
          @click="emit('action', b.id)"
        >
          {{ b.label }}
        </button>
      </div>
    </div>
  </div>
</template>
