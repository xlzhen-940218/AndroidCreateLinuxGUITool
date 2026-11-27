<script setup lang="ts">
import { t } from "../../i18n";
import { DESKTOP_PRESETS } from "../../types";
import { useApp } from "../../store";

const {
  stageCurrent,
  install,
  installRunning,
  detecting,
  installed,
  options,
  phaseLabel,
  busy,
  serial,
  done,
  startInstall,
  cancelInstall,
  detectInstalled,
  skipStage,
  refreshPhase,
} = useApp();
</script>

<template>
  <section v-if="stageCurrent" class="card">
    <h2>{{ t(`step.${stageCurrent}`) }}</h2>
    <p class="lead">{{ t(`stage.${stageCurrent}.lead`) }}</p>

    <div v-if="stageCurrent === 'base'" class="grid">
      <div class="field">
        <label>{{ t("inst.desktopParams") }}</label>
        <div class="row">
          <div class="sub">
            <span>{{ t("inst.sshPort") }}</span>
            <input v-model.number="options.sshPort" class="input" type="number" />
          </div>
        </div>
      </div>
    </div>

    <div v-if="stageCurrent === 'desktop'" class="grid">
      <div class="field">
        <label>{{ t("inst.desktop") }}</label>
        <div class="presets">
          <button
            v-for="p in DESKTOP_PRESETS"
            :key="p.id"
            class="preset"
            :class="{ on: options.desktop === p.id }"
            @click="options.desktop = p.id"
          >
            <strong>{{ p.name }}</strong>
            <em>{{ p.size }}</em>
            <span>{{ t(`preset.${p.id}`) }}</span>
          </button>
        </div>
      </div>
    </div>

    <div v-if="stageCurrent === 'apps'" class="grid">
      <div class="field">
        <label class="check">
          <input v-model="options.installApps" type="checkbox" />
          {{ t("inst.installApps") }}
        </label>
        <label class="check">
          <input v-model="options.installIme" type="checkbox" />
          {{ t("inst.installIme") }}
        </label>
      </div>
    </div>

    <div v-if="stageCurrent === 'vncsvc'" class="grid">
      <div class="field">
        <label>{{ t("inst.desktopParams") }}</label>
        <div class="row">
          <div class="sub">
            <span>{{ t("inst.vncPassword") }}</span>
            <input v-model="options.vncPassword" class="input" />
          </div>
          <div class="sub">
            <span>{{ t("inst.resolution") }}</span>
            <select v-model="options.vncGeometry" class="input">
              <option>1280x720</option>
              <option>1600x900</option>
              <option>1920x1080</option>
            </select>
          </div>
        </div>
      </div>
    </div>

    <div v-if="install.state !== 'idle'" class="statusbar" :class="install.state">
      <div class="status-main">
        <strong>{{ install.message || install.state }}</strong>
        <span v-if="install.step">{{ t("inst.step", { step: install.step }) }}</span>
      </div>
      <button v-if="installRunning" class="btn ghost" @click="cancelInstall()">
        {{ t("btn.cancel") }}
      </button>
    </div>

    <div class="inline-note">
      {{ t("inst.terminalStatus", { phase: phaseLabel }) }}
      <button class="link" @click="refreshPhase()">{{ t("btn.refresh") }}</button>
    </div>

    <div v-if="stageCurrent === 'base'" class="detect">
      <div class="detect-head">
        <span>{{ t("inst.detectTitle") }}</span>
        <button
          class="link"
          :disabled="detecting || installRunning || !serial"
          @click="detectInstalled(false)"
        >
          {{ detecting ? t("btn.detecting") : installed ? t("app.redetect") : t("btn.detectNow") }}
        </button>
      </div>
      <div v-if="detecting" class="detect-line">
        {{ t("inst.detectingHint") }}
      </div>
      <div v-else-if="installed" class="detect-grid">
        <span :class="installed.desktop ? 'ok' : 'miss'">
          {{ t("inst.desktopLabel") }}：{{ installed.desktop ? installed.desktopName : t("common.notDetected") }}
        </span>
        <span :class="installed.apps ? 'ok' : 'miss'">
          {{ t("inst.appsLabel") }}：{{ installed.appsInstalled }}/{{ installed.appsTotal }}
        </span>
        <span :class="installed.ime && installed.imeEnv ? 'ok' : 'miss'">
          {{ t("inst.imeLabel") }}：{{
            !installed.ime
              ? t("common.notInstalled")
              : installed.imeEnv
                ? t("common.installed")
                : t("inst.imeNoEnv")
          }}
        </span>
        <span :class="installed.vncPort ? 'ok' : 'miss'">
          {{ t("inst.vncLabel") }}：{{
            !installed.vnc
              ? t("common.notInstalled")
              : installed.vncPort
                ? t("common.running")
                : t("common.notRunning")
          }}
        </span>
        <span :class="installed.audioSink ? 'ok' : 'miss'">
          {{ t("audio.speakerLabel") }}：{{
            installed.audioSink ? installed.audioSinkName : t("audio.noSpeaker")
          }}
        </span>
      </div>
      <div v-else class="detect-line">
        {{ t("inst.detectHint") }}
      </div>
      <div v-if="installed && installed.ime && !installed.imeEnv" class="detect-tip">
        {{ t("inst.imeEnvTip") }}
      </div>
    </div>

    <div
      v-if="stageCurrent === 'base' && installed?.desktop && !done.desktop"
      class="detect-tip"
    >
      {{ t("inst.alreadyInstalled", { name: installed.desktopName }) }}
      <button class="link" @click="skipStage()">{{ t("btn.skipStage") }}</button>
    </div>

    <div class="actions">
      <button
        class="btn primary"
        :disabled="busy || installRunning || !serial"
        @click="startInstall(stageCurrent ?? 'base')"
      >
        {{
          installRunning
            ? t("btn.installing")
            : done[stageCurrent ?? "base"]
              ? t("btn.reinstall")
              : t("btn.startInstall")
        }}
      </button>
    </div>
  </section>
</template>
