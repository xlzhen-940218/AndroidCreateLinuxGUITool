<script setup lang="ts">
import { t } from "../../i18n";
import { useApp } from "../../store";

const { env, busy, adbPathInput, showAdbSettings, saveAdbPath, refreshDevices } = useApp();
</script>

<template>
  <section class="card">
    <h2>{{ t("env.title") }}</h2>
    <p class="lead">{{ t("env.lead") }}</p>
    <div class="row">
      <input v-model="adbPathInput" class="input" :placeholder="t('env.placeholder')" />
      <button class="btn" @click="saveAdbPath()">{{ t("btn.saveDetect") }}</button>
      <button class="btn ghost" @click="showAdbSettings = !showAdbSettings">
        {{ showAdbSettings ? t("btn.collapse") : t("btn.manualPath") }}
      </button>
    </div>
    <div v-if="env" class="kv">
      <div><span>{{ t("env.adbPath") }}</span><code>{{ env.adbPath || t("common.notFound") }}</code></div>
      <div><span>{{ t("env.version") }}</span><code>{{ env.adbVersion || env.error || "-" }}</code></div>
    </div>
    <div class="actions">
      <button class="btn primary" :disabled="busy || !env?.adbFound" @click="refreshDevices()">
        {{ t("btn.nextDevice") }}
      </button>
    </div>
  </section>
</template>
