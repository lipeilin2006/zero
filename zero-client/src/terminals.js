//! 多标签 / 多窗格终端（仿 Windows Terminal 的操作模型），基于 xterm.js。
//!
//! 一个标签（tab）里可以有多个窗格（pane），每个窗格是一条到
//! zero-server 的独立 PTY 会话。xterm.js 负责完整终端仿真，
//! 所以 vim / top / htop 这类全屏程序也能用。

import { reactive, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/// 标签列表
export const tabs = reactive([]);
/// 当前激活的标签
export const activeTab = ref(null);
/// 窗格里的 xterm 实例：termName -> { write, focus, fit, dispose }
const terminals = new Map();

let tabSeq = 0;
let paneSeq = 0;
let listening = false;

export function registerTerminal(termName, api) {
  terminals.set(termName, api);
  return () => {
    if (terminals.get(termName) === api) terminals.delete(termName);
  };
}

export function focusTerminal(termName) {
  terminals.get(termName)?.focus();
}

function writeText(pane, text) {
  terminals.get(pane.termName)?.write(text);
}

function writeBytes(pane, bytes) {
  terminals.get(pane.termName)?.write(bytes);
}

async function createPane(server) {
  const id = await invoke("session_open", {
    endpoint: server.endpoint,
    cols: 120,
    rows: 32,
  });
  return reactive({
    id,
    key: ++paneSeq,
    termName: `zero-${id}`,
    serverName: server.name,
    endpoint: server.endpoint,
    closed: false,
    exitCode: null,
    welcome: [`\x1b[32m已连接到 ${server.name}\x1b[0m`, `\x1b[90m${server.endpoint}\x1b[0m`].join(
      "\r\n",
    ),
  });
}

export async function openTab(server) {
  const pane = await createPane(server);
  const tab = reactive({
    id: ++tabSeq,
    title: server.name,
    endpoint: server.endpoint,
    panes: [pane],
    direction: "row",
    activePaneId: pane.id,
  });
  tabs.push(tab);
  activeTab.value = tab;
  return tab;
}

/// 在现有标签里加一个窗格。direction: "row"（左右）| "col"（上下）
export async function splitPane(tab, server, direction = "row") {
  if (!tab) return null;
  const pane = await createPane(server);
  tab.direction = direction;
  tab.panes.push(pane);
  tab.activePaneId = pane.id;
  return pane;
}

export function activePane(tab = activeTab.value) {
  if (!tab) return null;
  return tab.panes.find((pane) => pane.id === tab.activePaneId) || tab.panes[0] || null;
}

export function setActivePane(tab, pane) {
  if (!tab || !pane) return;
  tab.activePaneId = pane.id;
  focusTerminal(pane.termName);
}

export async function closePane(tab, pane) {
  if (!tab || !pane) return;
  const index = tab.panes.findIndex((item) => item.id === pane.id);
  if (index === -1) return;
  if (!pane.closed) {
    pane.closed = true;
    try {
      await invoke("session_close", { id: pane.id });
    } catch {
      // 后端已经收掉了也无所谓
    }
  }
  terminals.delete(pane.termName);
  tab.panes.splice(index, 1);
  if (tab.panes.length === 0) {
    await closeTab(tab);
    return;
  }
  const next = tab.panes[Math.min(index, tab.panes.length - 1)];
  tab.activePaneId = next.id;
  focusTerminal(next.termName);
}

export async function closeTab(tab) {
  if (!tab) return;
  const index = tabs.findIndex((item) => item.id === tab.id);
  if (index === -1) return;
  for (const pane of [...tab.panes]) {
    if (!pane.closed) {
      pane.closed = true;
      try {
        await invoke("session_close", { id: pane.id });
      } catch {
        // 忽略
      }
    }
    terminals.delete(pane.termName);
  }
  tabs.splice(index, 1);
  if (activeTab.value?.id === tab.id) {
    activeTab.value = tabs[index] || tabs[index - 1] || null;
    const next = activePane(activeTab.value);
    if (next) focusTerminal(next.termName);
  }
}

export function cycleTab(step) {
  if (tabs.length < 2) return;
  const index = tabs.findIndex((tab) => tab.id === activeTab.value?.id);
  const next = (index + step + tabs.length) % tabs.length;
  activeTab.value = tabs[next];
}

export async function sendInput(pane, data) {
  if (!pane || pane.closed) return;
  try {
    await invoke("session_input", { id: pane.id, data });
  } catch {
    // 会话刚关掉时发送会失败，忽略即可
  }
}

export async function sendControl(pane, sequence) {
  await sendInput(pane, sequence);
}

export async function resizeSession(pane, cols, rows) {
  if (!pane || pane.closed) return;
  try {
    await invoke("session_resize", {
      id: pane.id,
      cols: Math.max(2, cols),
      rows: Math.max(1, rows),
    });
  } catch {
    // 尺寸变化失败不影响使用
  }
}

/// 全局只注册一次会话事件。
export async function initSessionEvents() {
  if (listening) return;
  listening = true;
  await listen("zero:session", (event) => {
    const payload = event.payload || {};
    let pane = null;
    for (const tab of tabs) {
      pane = tab.panes.find((item) => item.id === payload.id);
      if (pane) break;
    }
    if (!pane) return;
    if (payload.kind === "data") {
      try {
        const raw = atob(payload.data);
        const bytes = new Uint8Array(raw.length);
        for (let index = 0; index < raw.length; index += 1) {
          bytes[index] = raw.charCodeAt(index);
        }
        writeBytes(pane, bytes);
      } catch {
        // 单条数据坏了就跳过
      }
    } else if (payload.kind === "exit") {
      pane.closed = true;
      pane.exitCode = payload.code;
      writeText(pane, `\r\n\x1b[90m—— 会话结束（退出码 ${payload.code}）——\x1b[0m\r\n`);
    } else if (payload.kind === "error") {
      pane.closed = true;
      writeText(pane, `\r\n\x1b[31m—— 连接出错：${payload.message} ——\x1b[0m\r\n`);
    }
  });
}
