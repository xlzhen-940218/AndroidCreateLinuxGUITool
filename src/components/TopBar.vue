<script setup lang="ts">
import { LOCALES, locale, t } from "../i18n";
import { useApp } from "../store";

const {
  env,
  devices,
  serial,
  busy,
  theme,
  themeIds,
  setTheme,
  onLocaleChange,
  refreshDevices,
  runAvfCheck,
  openRemoteDesktop,
} = useApp();
</script>

<template>
  <header class="topbar">
    <div class="brand">
      <div class="logo">AVF</div>
      <div>
        <h1>{{ t("app.title") }}</h1>
        <p>{{ t("app.subtitle") }}</p>
      </div>
    </div>
    <div class="topbar-right">
      <div class="theme-switch" :title="t('theme.label')">
        <button
          v-for="id in themeIds"
          :key="id"
          :class="{ on: theme === id }"
          @click="setTheme(id)"
        >
          <span class="dot" :class="id"></span>{{ t(`theme.${id}`) }}
        </button>
      </div>
      <select class="select" :value="locale" @change="onLocaleChange">
        <option v-for="l in LOCALES" :key="l.id" :value="l.id">{{ l.label }}</option>
      </select>
      <div v-if="env" class="pill" :class="env.adbFound ? 'ok' : 'bad'">
        {{
          env.adbFound
            ? t("app.adbReady", { version: env.adbVersion || "OK" })
            : t("app.adbNotReady")
        }}
      </div>
      <select v-if="devices.length" v-model="serial" class="select" @change="runAvfCheck()">
        <option v-for="d in devices" :key="d.serial" :value="d.serial">
          {{ d.model || d.product || d.serial }} · {{ d.serial }} ({{ d.state }})
        </option>
      </select>
      <button class="btn ghost" :disabled="busy" @click="refreshDevices()">
        {{ t("app.redetect") }}
      </button>
      <button
        class="btn primary quick-remote"
        :title="t('topbar.remoteDesktopHint')"
        @click="openRemoteDesktop()"
      >
        <span class="quick-remote-icon">▷</span>{{ t("topbar.remoteDesktop") }}
      </button>
    </div>
  </header>
</template>
