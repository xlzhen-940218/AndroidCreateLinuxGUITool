<script setup lang="ts">
import { t } from "../../i18n";
import { useApp } from "../../store";

const { devices, serial, readyDevice, busy, refreshDevices, runAvfCheck } = useApp();
</script>

<template>
  <section class="card">
    <h2>{{ t("device.title") }}</h2>
    <p class="lead">{{ t("device.connected") }}</p>
    <div v-if="devices.length === 0" class="empty">{{ t("device.none") }}</div>
    <table v-else class="table">
      <thead>
        <tr>
          <th>{{ t("device.colSerial") }}</th>
          <th>{{ t("device.colModel") }}</th>
          <th>{{ t("device.colState") }}</th>
          <th>{{ t("device.colProduct") }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="d in devices" :key="d.serial" :class="{ picked: d.serial === serial }">
          <td><code>{{ d.serial }}</code></td>
          <td>{{ d.model || "-" }}</td>
          <td><span class="tag" :class="d.state === 'device' ? 'ok' : 'bad'">{{ d.state }}</span></td>
          <td>{{ d.product || "-" }}</td>
        </tr>
      </tbody>
    </table>
    <div class="actions">
      <button class="btn ghost" :disabled="busy" @click="refreshDevices()">{{ t("app.redetect") }}</button>
      <button class="btn primary" :disabled="busy || !readyDevice" @click="runAvfCheck()">
        {{ t("btn.nextAvf") }}
      </button>
    </div>
  </section>
</template>
