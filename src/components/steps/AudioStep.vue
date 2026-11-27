<script setup lang="ts">
import { t } from "../../i18n";
import { useApp } from "../../store";

const { audio, audioProgress, busy, serial, setupAudio, detectAudio, openAudioPlayer } = useApp();
</script>

<template>
  <section class="card">
    <h2>{{ t("audio.title") }}</h2>
    <p class="lead">{{ t("audio.lead") }}</p>
    <div v-if="audio" class="kv">
      <div>
        <span>{{ t("audio.speakerLabel") }}</span>
        <code>{{
          audio.hasSpeaker
            ? t("audio.hasSpeaker", { name: audio.sinkName })
            : t("audio.noSpeaker")
        }}</code>
      </div>
      <div v-if="!audio.hasSpeaker">
        <span>{{ t("audio.connectLabel") }}</span>
        <code>{{ audio.connectHost }}:{{ audio.connectPort }}</code>
      </div>
      <div v-if="!audio.hasSpeaker">
        <span>{{ t("audio.formatLabel") }}</span>
        <code>{{ audio.rate }} Hz · {{ audio.channels === 2 ? "stereo" : "mono" }}</code>
      </div>
      <div v-if="!audio.hasSpeaker">
        <span>{{ t("audio.reachableLabel") }}</span>
        <code>
          {{ audio.reachable ? t("audio.reachableOk") : t("audio.reachableFail") }}
          <template v-if="audio.reachable"> · {{ audio.viaRelay ? t("audio.viaRelay") : t("audio.direct") }}</template>
        </code>
      </div>
      <div v-if="!audio.hasSpeaker">
        <span>{{ t("audio.playerLabel") }}</span>
        <code>{{
          audio.playerInstalled
            ? audio.streaming
              ? t("audio.streaming")
              : t("audio.playerReady")
            : t("audio.playerMissing")
        }}</code>
      </div>
    </div>
    <p v-if="audioProgress" class="lead">{{ audioProgress }}</p>
    <div class="hintbox">
      <strong>{{ t("audio.phoneBox") }}</strong>
      <ul>
        <li>{{ t("audio.step1") }}</li>
        <li>{{ t("audio.step2") }}</li>
        <li>{{ t("audio.step3") }}</li>
      </ul>
    </div>
    <div class="actions">
      <button class="btn primary" :disabled="busy || !serial" @click="setupAudio()">
        {{ t("btn.setupAudio") }}
      </button>
      <button
        class="btn"
        :disabled="busy || !serial || !audio || audio.hasSpeaker"
        @click="openAudioPlayer()"
      >
        {{ t("audio.openPlayer") }}
      </button>
      <button class="btn ghost" :disabled="busy || !serial" @click="detectAudio()">
        {{ t("btn.detectAudio") }}
      </button>
    </div>
  </section>
</template>
