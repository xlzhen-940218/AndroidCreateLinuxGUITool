<script setup lang="ts">
import { t } from "../../i18n";
import { useApp } from "../../store";

const {
  phaseLabel,
  imageChecking,
  imageStatus,
  options,
  busy,
  serial,
  current,
  stepIndex,
  installImage,
  detectImage,
} = useApp();
</script>

<template>
  <section class="card">
    <h2>{{ t("image.title") }}</h2>
    <p class="lead">{{ t("image.lead") }}</p>
    <div class="kv">
      <div><span>{{ t("image.phase") }}</span><code>{{ phaseLabel }}</code></div>
      <div v-if="imageChecking">
        <span>{{ t("image.checkLabel") }}</span><code>{{ t("image.detecting") }}</code>
      </div>
      <div v-else-if="imageStatus?.message">
        <span>{{ t("image.checkLabel") }}</span><code>{{ imageStatus.message }}</code>
      </div>
    </div>
    <ul class="notes">
      <li>{{ t("image.note1") }}</li>
      <li>{{ t("image.note2") }}</li>
    </ul>
    <div class="grid">
      <div class="field">
        <label>{{ t("image.diskLabel") }}</label>
        <div class="row">
          <div class="sub">
            <span>{{ t("image.diskSizeLabel") }}</span>
            <input v-model.number="options.diskSizeGb" class="input" type="number" min="0" max="512" />
          </div>
          <div class="sub">
            <span>{{ t("image.diskHint") }}</span>
          </div>
        </div>
      </div>
    </div>
    <div class="actions">
      <button class="btn primary" :disabled="busy || imageChecking || !serial" @click="installImage()">
        {{ busy ? t("image.working") : t("image.start") }}
      </button>
      <button class="btn ghost" :disabled="busy || imageChecking || !serial" @click="detectImage()">
        {{ imageChecking ? t("image.detecting") : t("image.detect") }}
      </button>
      <button
        class="btn ghost"
        :disabled="busy || imageStatus?.phase !== 'ready'"
        @click="current = stepIndex('base')"
      >
        {{ t("btn.continueInstall") }}
      </button>
    </div>
  </section>
</template>
