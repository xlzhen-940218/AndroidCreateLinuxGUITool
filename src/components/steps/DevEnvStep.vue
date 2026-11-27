<script setup lang="ts">
import { api } from "../../api";
import { t } from "../../i18n";
import { useApp } from "../../store";

const { devopts, busy, serial, captureScreen, runDevOptions, runEnableLinuxEnv } = useApp();
</script>

<template>
  <section class="card">
    <h2>{{ t("devenv.title") }}</h2>
    <p class="lead">{{ t("devenv.lead") }}</p>
    <div v-if="devopts" class="kv">
      <div><span>{{ t("devenv.devOptions") }}</span><code>{{ devopts.devOptionsEnabled ? t("devenv.on") : t("devenv.off") }}</code></div>
      <div><span>{{ t("devenv.usb") }}</span><code>{{ devopts.adbEnabled ? t("devenv.on") : t("devenv.off") }}</code></div>
      <div><span>{{ t("devenv.linuxEnv") }}</span><code>{{ devopts.linuxEnvEnabled ? t("devenv.on") : t("devenv.off") }}</code></div>
      <div v-if="devopts.linuxEntryVisible">
        <span>{{ t("devenv.entry") }}</span>
        <code>{{ devopts.linuxEntryLabel }}<template v-if="devopts.linuxSwitchChecked !== null"> · {{ devopts.linuxSwitchChecked ? t("devenv.switchOn") : t("devenv.switchOff") }}</template></code>
      </div>
      <div><span>{{ t("devenv.model") }}</span><code>{{ devopts.isPixel ? t("devenv.pixel") : t("devenv.nonPixel") }}</code></div>
      <div v-if="devopts.devicePolicyPresent"><span>{{ t("devenv.policy") }}</span><code>{{ devopts.devicePolicyDetail }}</code></div>
    </div>
    <ul v-if="devopts?.notes.length" class="notes">
      <li v-for="(n, i) in devopts.notes" :key="i">{{ n }}</li>
    </ul>
    <div class="actions">
      <button class="btn ghost" :disabled="busy" @click="api.openDevOptions(serial)">
        {{ t("btn.openDevOnPhone") }}
      </button>
      <button class="btn ghost" :disabled="busy || !serial" @click="captureScreen()">
        {{ t("btn.capture") }}
      </button>
      <button class="btn ghost" :disabled="busy" @click="runDevOptions()">{{ t("app.redetect") }}</button>
      <button
        class="btn primary"
        :disabled="busy || !serial || devopts?.linuxEnvEnabled"
        @click="runEnableLinuxEnv()"
      >
        {{ devopts?.linuxEnvEnabled ? t("btn.enabled") : t("btn.enableLinux") }}
      </button>
    </div>
  </section>
</template>
