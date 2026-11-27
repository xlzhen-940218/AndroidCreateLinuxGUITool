<script setup lang="ts">
import { computed, onMounted, onUnmounted } from "vue";
import LogPanel from "./components/LogPanel.vue";
import ModalDialog from "./components/ModalDialog.vue";
import StepRail from "./components/StepRail.vue";
import TopBar from "./components/TopBar.vue";
import AudioStep from "./components/steps/AudioStep.vue";
import AvfStep from "./components/steps/AvfStep.vue";
import DevEnvStep from "./components/steps/DevEnvStep.vue";
import DeviceStep from "./components/steps/DeviceStep.vue";
import EnvStep from "./components/steps/EnvStep.vue";
import ImageStep from "./components/steps/ImageStep.vue";
import InstallStageStep from "./components/steps/InstallStageStep.vue";
import VncStep from "./components/steps/VncStep.vue";
import { isStage, useApp } from "./store";

const { currentStepId, logs, dialog, onDialogAction, init, dispose } = useApp();
const isStageStep = computed(() => isStage(currentStepId.value));

onMounted(() => {
  void init();
});

onUnmounted(() => dispose());
</script>

<template>
  <div class="app">
    <TopBar />
    <div class="body">
      <StepRail />
      <main class="panel">
        <EnvStep v-if="currentStepId === 'env'" />
        <DeviceStep v-else-if="currentStepId === 'device'" />
        <AvfStep v-else-if="currentStepId === 'avf'" />
        <DevEnvStep v-else-if="currentStepId === 'devenv'" />
        <ImageStep v-else-if="currentStepId === 'image'" />
        <InstallStageStep v-else-if="isStageStep" />
        <AudioStep v-else-if="currentStepId === 'audio'" />
        <VncStep v-else-if="currentStepId === 'vnc'" />

        <LogPanel :lines="logs" />
      </main>
    </div>

    <ModalDialog
      v-if="dialog"
      :title="dialog.title"
      :message="dialog.message"
      :detail="dialog.detail"
      :tone="dialog.tone"
      :buttons="dialog.buttons"
      @action="onDialogAction"
    />
  </div>
</template>
