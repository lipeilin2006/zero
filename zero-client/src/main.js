import { createApp } from "vue";
import App from "./App.vue";

const app = createApp(App);
app.config.errorHandler = (error, _instance, info) => {
  console.error("vue error:", error, info);
  window.dispatchEvent(
    new CustomEvent("zero:ui-error", {
      detail: `${error?.message || error} (${info})`,
    }),
  );
};

app.mount("#app");
