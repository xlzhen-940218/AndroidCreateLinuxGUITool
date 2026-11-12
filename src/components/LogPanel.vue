<script setup lang="ts">
import { nextTick, ref, watch } from "vue";

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
      <span>{{ title ?? "执行日志" }}</span>
      <span class="log-count">{{ lines.length }} 行</span>
    </div>
    <div ref="box" class="log-body">
      <div v-if="lines.length === 0" class="log-empty">等待输出…</div>
      <div v-for="(l, i) in lines" :key="i" class="log-line">{{ l }}</div>
    </div>
  </div>
</template>
