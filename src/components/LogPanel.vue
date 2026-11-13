<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { t } from "../i18n";

const props = defineProps<{ lines: string[]; title?: string }>();
const box = ref<HTMLElement | null>(null);

watch(
  () => props.lines.length,
  async () => {
    await nextTick();
    if (box.value) box.value.scrollTop = box.value.scrollHeight;
  },
);
</script>

<template>
  <div class="log-panel">
    <div class="log-head">
      <span>{{ title ?? t("log.title") }}</span>
      <span class="log-count">{{ t("log.lines", { n: lines.length }) }}</span>
    </div>
    <div ref="box" class="log-body">
      <div v-if="lines.length === 0" class="log-empty">{{ t("log.waiting") }}</div>
      <div v-for="(l, i) in lines" :key="i" class="log-line">{{ l }}</div>
    </div>
  </div>
</template>
