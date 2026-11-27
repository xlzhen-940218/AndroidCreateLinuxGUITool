<script setup lang="ts">
import { t } from "../../i18n";
import { useApp } from "../../store";

const { avf, busy, runAvfCheck, runDevOptions } = useApp();
</script>

<template>
  <section class="card">
    <h2>{{ t("avf.title") }}</h2>
    <p class="lead">
      {{ t("avf.lead") }}<code>adb shell /apex/com.android.virt/bin/vm info</code>
      <code>adb shell pm list features | grep virtualization_framework</code>
    </p>
    <div v-if="avf" class="kv">
      <div><span>{{ t("avf.device") }}</span><code>{{ avf.manufacturer }} {{ avf.model }}</code></div>
      <div><span>{{ t("avf.system") }}</span><code>Android {{ avf.androidRelease }} (API {{ avf.sdkInt }})</code></div>
      <div><span>{{ t("avf.feature") }}</span><code>{{ avf.featureLine || t("avf.featureMiss") }}</code></div>
      <div><span>{{ t("avf.runtime") }}</span><code>{{ avf.virtApexPresent ? t("avf.apexOk") : t("avf.missing") }}</code></div>
      <div><span>{{ t("avf.vmInfo") }}</span><code>{{ avf.vmInfoOk ? t("avf.vmOk") : t("avf.vmFail") }}</code></div>
      <div>
        <span>{{ t("avf.terminal") }}</span>
        <code>{{ avf.terminalPkgPresent ? (avf.terminalPkgEnabled ? t("avf.termOk") : t("avf.termDisabled")) : t("common.notInstalled") }}</code>
      </div>
    </div>
    <pre v-if="avf?.vmInfo" class="codeblock">{{ avf.vmInfo }}</pre>
    <div class="actions">
      <button class="btn ghost" :disabled="busy" @click="runAvfCheck()">{{ t("app.redetect") }}</button>
      <button class="btn primary" :disabled="busy || !avf?.supported" @click="runDevOptions()">
        {{ t("btn.nextDevenv") }}
      </button>
    </div>
  </section>
</template>
