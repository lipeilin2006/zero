<script setup>
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import "@xterm/xterm/css/xterm.css";
import { registerTerminal, resizeSession, sendInput, uiLog } from "../terminals";

const props = defineProps({
  pane: { type: Object, required: true },
  active: { type: Boolean, default: false },
});

const host = ref(null);
let term = null;
let fit = null;
let observer = null;
let layoutObserver = null;
let layoutTimer = null;
let disposers = [];
let unregister = null;
let started = false;
let fitFrame = null;
let lastSize = { width: 0, height: 0 };

function tryFit() {
  if (!fit || !host.value) return;
  const { clientWidth, clientHeight } = host.value;
  if (clientWidth < 8 || clientHeight < 8) return;
  try {
    fit.fit();
  } catch {
    // 尺寸为 0 时 fit 会抛，忽略
  }
}

/// 第一个标签页挂载时，终端区域可能刚从 display:none 变为可见，
/// 尺寸还是 0。等它真正有布局了再创建 xterm，否则渲染器会按 0 尺寸初始化。
function whenLaidOut(callback) {
  const element = host.value;
  if (!element) return;
  if (element.clientWidth >= 8 && element.clientHeight >= 8) {
    callback();
    return;
  }
  layoutObserver = new ResizeObserver(() => {
    if (!host.value) return;
    if (host.value.clientWidth >= 8 && host.value.clientHeight >= 8) {
      layoutObserver?.disconnect();
      layoutObserver = null;
      if (layoutTimer) {
        clearTimeout(layoutTimer);
        layoutTimer = null;
      }
      callback();
    }
  });
  layoutObserver.observe(element);
  // 兜底：3 秒还没布局就先建起来，后面 fit 会纠正。
  layoutTimer = setTimeout(() => {
    layoutObserver?.disconnect();
    layoutObserver = null;
    callback();
  }, 3000);
}

function initTerminal() {
  if (started || !host.value) return;
  started = true;

  term = new Terminal({
    fontSize: 13,
    lineHeight: 1.25,
    // 末尾带一组 CJK/emoji 兜底字体：缺字时浏览器会自动回退，
    // 不会出现方框或错位。
    fontFamily:
      '"JetBrains Mono", "Cascadia Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC", "Noto Color Emoji", monospace',
    cursorBlink: true,
    scrollback: 8000,
    // unicode11 字宽表属于 proposed API，必须放开
    allowProposedApi: true,
    theme: {
      background: "#0d1117",
      foreground: "#d5dde8",
      cursor: "#5eead4",
      selectionBackground: "rgba(94, 234, 212, 0.25)",
      black: "#11161d",
      red: "#f87171",
      green: "#6ee7a8",
      yellow: "#fbbf24",
      blue: "#7dd3fc",
      magenta: "#c4b5fd",
      cyan: "#5eead4",
      white: "#e6edf5",
      brightBlack: "#5d6b7d",
      brightRed: "#fca5a5",
      brightGreen: "#8af0bc",
      brightYellow: "#fcd34d",
      brightBlue: "#9be0ff",
      brightMagenta: "#ddd6fe",
      brightCyan: "#7df0dd",
      brightWhite: "#f4f8fc",
    },
  });

  fit = new FitAddon();
  term.loadAddon(fit);

  // 宽字符（中文 / emoji）用 Unicode 11 的字宽表，避免列对不齐。
  const unicode = new Unicode11Addon();
  term.loadAddon(unicode);
  term.unicode.activeVersion = "11";

  term.open(host.value);
  uiLog(
    `term.open 完成，host=${host.value?.clientWidth}x${host.value?.clientHeight}`,
  );
  term.write(props.pane.welcome + "\r\n");
  tryFit();
  uiLog(`fit 完成，cols=${term.cols} rows=${term.rows}`);

  disposers.push(
    term.onData((data) => {
      if (!props.pane.firstInput) {
        props.pane.firstInput = true;
        uiLog(`首个输入 ${props.pane.termName} len=${data.length}`);
      }
      sendInput(props.pane, data);
    }),
  );
  disposers.push(
    term.onResize(({ cols, rows }) => {
      resizeSession(props.pane, cols, rows);
    }),
  );

  unregister = registerTerminal(props.pane.termName, {
    write: (data) => term?.write(data),
    focus: () => term?.focus(),
    fit: tryFit,
    dispose: () => term?.dispose(),
  });

  // 合并同帧内的多次尺寸变化，避免 fit ↔ 容器尺寸互相触发形成抖动。
  observer = new ResizeObserver(() => {
    if (!props.active) return;
    if (fitFrame) return;
    fitFrame = requestAnimationFrame(() => {
      fitFrame = null;
      const { clientWidth, clientHeight } = host.value || { clientWidth: 0, clientHeight: 0 };
      if (clientWidth === lastSize.width && clientHeight === lastSize.height) return;
      lastSize = { width: clientWidth, height: clientHeight };
      tryFit();
    });
  });
  observer.observe(host.value);

  if (props.active) {
    nextTick(() => {
      tryFit();
      term?.focus();
    });
  }
}

onMounted(() => {
  uiLog(`XTermPane mount ${props.pane.termName}`);
  whenLaidOut(() => {
    try {
      initTerminal();
    } catch (error) {
      uiLog(`xterm 初始化失败: ${error?.stack || error}`);
      if (host.value) {
        host.value.textContent = `终端初始化失败：${error?.message || error}`;
        host.value.style.color = "#f87171";
        host.value.style.fontSize = "12px";
        host.value.style.padding = "12px";
      }
    }
  });
});

watch(
  () => props.active,
  async (active) => {
    if (!active) return;
    await nextTick();
    tryFit();
    term?.focus();
  },
);

onBeforeUnmount(() => {
  if (fitFrame) cancelAnimationFrame(fitFrame);
  if (layoutTimer) clearTimeout(layoutTimer);
  layoutObserver?.disconnect();
  observer?.disconnect();
  for (const disposable of disposers) disposable?.dispose?.();
  disposers = [];
  unregister?.();
  unregister = null;
  term?.dispose();
  term = null;
});
</script>

<template>
  <div ref="host" class="xterm-host"></div>
</template>

<style scoped>
.xterm-host {
  width: 100%;
  height: 100%;
  padding: 8px 10px;
  background: #0d1117;
  box-sizing: border-box;
  overflow: hidden;
}

.xterm-host :deep(.xterm) {
  height: 100%;
}

.xterm-host :deep(.xterm-viewport) {
  scrollbar-width: thin;
}
</style>
