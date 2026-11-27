<script setup lang="ts">
import { t } from "../i18n";
import { useApp } from "../store";

const { busy, stepGroups, stepState, stepIndex, goTo, openRemoteDesktop } = useApp();
</script>

<template>
  <aside class="rail">
    <div v-for="group in stepGroups" :key="group.id" class="step-group">
      <div class="step-group-title">{{ t(group.titleKey) }}</div>
      <ol class="steps">
        <li
          v-for="id in group.steps"
          :key="id"
          class="step"
          :class="stepState(id)"
          @click="goTo(stepIndex(id))"
        >
          <span class="step-dot">
            <template v-if="stepState(id) === 'done'">✓</template>
            <template v-else-if="stepState(id) === 'failed'">!</template>
            <template v-else>{{ stepIndex(id) + 1 }}</template>
          </span>
          <span class="step-text">
            <strong>{{ t(`step.${id}`) }}</strong>
            <em>{{ t(`step.${id}.desc`) }}</em>
          </span>
          <span v-if="id === 'vnc'" class="step-badge">{{ t("vnc.quickBadge") }}</span>
        </li>
      </ol>
    </div>

    <button class="rail-quick" :disabled="busy" @click="openRemoteDesktop()">
      <span class="rail-quick-icon">▷</span>
      <span class="rail-quick-text">
        <strong>{{ t("topbar.remoteDesktop") }}</strong>
        <em>{{ t("vnc.quickHint") }}</em>
      </span>
    </button>
  </aside>
</template>
