<script setup lang="ts">
import { api } from "../../api";
import { t } from "../../i18n";
import { useApp } from "../../store";

const { vnc, vncProgress, busy, serial, options, enableVncPort, installAvnc } = useApp();
</script>

<template>
  <section class="card">
    <h2>{{ t("vnc.title") }}</h2>
    <p class="lead">{{ t("vnc.lead") }}</p>
    <div v-if="vnc" class="kv">
      <div><span>{{ t("vnc.port") }}</span><code>{{ vnc.relayDetail }}</code></div>
      <div><span>{{ t("vnc.forward") }}</span><code>{{ vnc.forwardDetail }}</code></div>
      <div>
        <span>{{ t("vnc.probe") }}</span>
        <code>{{ vnc.reachable ? `${t("vnc.probeOk")} — ${vnc.probeDetail}` : `${t("vnc.probeFail")} — ${vnc.probeDetail}` }}</code>
      </div>
      <div><span>{{ t("vnc.client") }}</span><code>{{ vnc.viewerPath || t("vnc.viewerMissing") }}</code></div>
      <div><span>{{ t("vnc.address") }}</span><code>{{ vnc.connectHint }}</code></div>
    </div>
    <p v-if="vncProgress" class="lead">{{ vncProgress }}</p>
    <div class="hintbox">
      <strong>{{ t("vnc.phoneBox") }}</strong>
      <ul>
        <li>{{ t("vnc.addrLabel") }}<code>127.0.0.1</code>　{{ t("vnc.portLabel") }}<code>5901</code></li>
        <li>{{ t("vnc.userLabel") }}<code>droid</code>（{{ t("vnc.passLabel") }}<code>{{ options.vncPassword }}</code>）</li>
        <li>{{ t("vnc.phoneHint") }}</li>
      </ul>
    </div>
    <div class="actions">
      <button class="btn primary" :disabled="busy || !serial" @click="enableVncPort()">
        {{ t("btn.enablePort") }}
      </button>
      <button class="btn" :disabled="busy || !serial" @click="installAvnc()">
        {{ busy ? t("btn.installingAvnc") : t("btn.installAvnc") }}
      </button>
      <button class="btn ghost" @click="api.openExternal('https://tigervnc.org/download/')">
        {{ t("btn.openTigerVnc") }}
      </button>
    </div>
  </section>
</template>
