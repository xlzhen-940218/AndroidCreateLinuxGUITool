import { createApp } from "vue";
import App from "./App.vue";
import "./styles.css";

// 在挂载前先应用主题，避免启动时闪一下默认配色
const savedTheme = localStorage.getItem("acl.theme");
document.documentElement.dataset.theme = savedTheme || "blue";

createApp(App).mount("#app");
