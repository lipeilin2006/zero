import { createApp } from "vue";
import App from "./App.vue";
import naive from "naive-ui";

const app = createApp(App);
app.use(naive);
app.config.errorHandler = (error, _instance, info) => {
  console.error("vue error:", error, info);
  window.dispatchEvent(
    new CustomEvent("zero:ui-error", {
      detail: `${error?.message || error} (${info})`,
    }),
  );
};

app.mount("#app");
