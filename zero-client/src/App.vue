<script setup>
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
// xterm 体积不小，等真正开终端时再加载
const XTermPane = defineAsyncComponent(() => import("./components/XTermPane.vue"));
import {
  activePane,
  activeTab,
  closePane,
  closeTab,
  cycleTab,
  focusTerminal,
  initSessionEvents,
  openTab,
  sendControl,
  setActivePane,
  splitPane,
  tabs,
  uiLog,
} from "./terminals";

const servers = ref([]);
const selectedIndex = ref(null);
const search = ref("");
const modalOpen = ref(false);
const draft = ref({ name: "", endpoint: "", tags: "" });
const deleteTarget = ref(null);
const deleteOpen = ref(false);
const pickerOpen = ref(false);
const splitMode = ref(null); // null | "row" | "col"
const busy = ref(false);
const status = ref("");

const themeOverrides = {
  common: {
    fontFamily: 'Inter, "Microsoft YaHei", "Noto Sans CJK SC", sans-serif',
    bodyColor: "#0b0f14",
    cardColor: "#11161d",
    modalColor: "#131922",
    popoverColor: "#131922",
    tableColor: "#11161d",
    actionBarColor: "#11161d",
    inputColor: "#0f141b",
    actionColor: "#1a2230",
    textColorBase: "#e6edf5",
    textColor1: "#e6edf5",
    textColor2: "#c3ccd8",
    textColor3: "#8794a5",
    placeholderColor: "#5d6b7d",
    borderColor: "#243041",
    dividerColor: "#1c2533",
    primaryColor: "#5eead4",
    primaryColorHover: "#7df0dd",
    primaryColorPressed: "#2dd4bf",
    primaryColorSuppl: "#5eead4",
    infoColor: "#7dd3fc",
    infoColorHover: "#9be0ff",
    infoColorPressed: "#38bdf8",
    successColor: "#6ee7a8",
    successColorHover: "#8af0bc",
    successColorPressed: "#34d399",
    warningColor: "#fbbf24",
    warningColorHover: "#fcd34d",
    warningColorPressed: "#f59e0b",
    errorColor: "#f87171",
    errorColorHover: "#fca5a5",
    errorColorPressed: "#ef4444",
    borderRadius: "8px",
    borderRadiusSmall: "6px",
  },
  Button: {
    fontWeightStrong: "600",
    heightMedium: "32px",
    heightSmall: "26px",
    paddingMedium: "0 14px",
    paddingSmall: "0 10px",
  },
  Input: {
    color: "#0f141b",
    colorFocus: "#111820",
    border: "1px solid #243041",
    borderHover: "1px solid #33445c",
    borderFocus: "1px solid #5eead4",
    boxShadowFocus: "0 0 0 2px rgba(94, 234, 212, .18)",
  },
  Card: {
    color: "#11161d",
    borderColor: "#1e2836",
    titleFontSizeMedium: "13px",
    titleFontWeight: "600",
  },
  Tag: {
    color: "#16202c",
    colorBordered: "#16202c",
    textColor: "#c3ccd8",
    textColorBordered: "#c3ccd8",
    borderColor: "#243041",
  },
  Scrollbar: {
    color: "rgba(94, 234, 212, .22)",
    colorHover: "rgba(94, 234, 212, .38)",
  },
  Dialog: {
    color: "#131922",
    titleFontSize: "15px",
    titleFontWeight: "600",
  },
  Popover: { color: "#16202c" },
};

const filteredServers = computed(() => {
  const query = search.value.trim().toLowerCase();
  return servers.value
    .map((server, index) => ({ server, index }))
    .filter(({ server }) => {
      if (!query) return true;
      return [server.name, server.endpoint, ...(server.tags || [])]
        .join(" ")
        .toLowerCase()
        .includes(query);
    })
    .sort((a, b) => {
      const recent = (b.server.last_opened || 0) - (a.server.last_opened || 0);
      return recent || a.server.name.localeCompare(b.server.name);
    });
});

const selected = computed(() =>
  selectedIndex.value === null ? null : servers.value[selectedIndex.value] || null,
);

function setStatus(message) {
  status.value = message;
}

async function loadServers() {
  try {
    servers.value = await invoke("list_servers");
  } catch (error) {
    setStatus(`加载失败：${error}`);
  }
}

function selectServer(index) {
  selectedIndex.value = index;
  activeTab.value = null;
}

function openCreate() {
  selectedIndex.value = null;
  draft.value = { name: "", endpoint: "", tags: "" };
  modalOpen.value = true;
}

function openEdit() {
  if (!selected.value) return;
  draft.value = {
    name: selected.value.name,
    endpoint: selected.value.endpoint,
    tags: (selected.value.tags || []).join(", "),
  };
  modalOpen.value = true;
}

async function saveServer() {
  const name = draft.value.name.trim();
  const endpoint = draft.value.endpoint.trim();
  if (!name || !endpoint) {
    setStatus("名称和 Endpoint ID 不能为空");
    return;
  }
  const tags = draft.value.tags
    .split(",")
    .map((tag) => tag.trim())
    .filter(Boolean);
  const server = {
    name,
    endpoint,
    tags,
    last_opened: selected.value?.last_opened || null,
  };
  if (selectedIndex.value === null) {
    servers.value.push(server);
    selectedIndex.value = servers.value.length - 1;
  } else {
    servers.value[selectedIndex.value] = server;
  }
  try {
    await invoke("save_servers", { servers: servers.value });
    modalOpen.value = false;
    setStatus(`已保存：${name}`);
  } catch (error) {
    setStatus(`保存失败：${error}`);
  }
}

async function removeServer() {
  if (deleteTarget.value === null) return;
  const removed = servers.value[deleteTarget.value];
  servers.value.splice(deleteTarget.value, 1);
  // 这条服务器还开着终端的话一起关掉
  if (removed) {
    for (const tab of [...tabs]) {
      if (tab.endpoint === removed.endpoint) await closeTab(tab);
    }
  }
  selectedIndex.value = null;
  deleteTarget.value = null;
  deleteOpen.value = false;
  await invoke("save_servers", { servers: servers.value });
  setStatus("服务器已删除");
}

async function openServerTab(server) {
  if (!server) return;
  uiLog(`openServerTab ${server.name}`);
  busy.value = true;
  const splitting = splitMode.value && activeTab.value;
  setStatus(splitting ? `正在拆分到 ${server.name}…` : `正在连接 ${server.name}…`);
  try {
    if (splitting) {
      await splitPane(activeTab.value, server, splitMode.value);
    } else {
      await openTab(server);
    }
    // 会话已经建好，先把加载状态收掉；保存列表放后台，别让它卡住按钮。
    busy.value = false;
    uiLog("openTab 完成");
    server.last_opened = Math.floor(Date.now() / 1000);
    invoke("save_servers", { servers: servers.value })
      .then(() => uiLog("save_servers 完成"))
      .catch(() => {});
    setStatus(splitting ? `已拆分：${server.name}` : `已连接：${server.name}`);
    pickerOpen.value = false;
    splitMode.value = null;
    await nextTick();
    focusActive();
  } catch (error) {
    setStatus(`连接失败：${error}`);
  } finally {
    busy.value = false;
  }
}

const plusOptions = computed(() => {
  const items = servers.value.map((server, index) => ({
    label: `${server.name}`,
    key: `srv:${index}`,
  }));
  if (items.length) items.push({ type: "divider", key: "divider" });
  items.push({ label: "管理服务器…", key: "manage" });
  return items;
});

function onPlusSelect(key) {
  if (key === "manage") {
    splitMode.value = null;
    pickerOpen.value = true;
    return;
  }
  const index = Number(String(key).split(":")[1]);
  if (Number.isInteger(index) && servers.value[index]) {
    openServerTab(servers.value[index]);
  }
}

function requestSplit(direction) {
  if (!activeTab.value) {
    pickerOpen.value = true;
    return;
  }
  splitMode.value = direction;
  pickerOpen.value = true;
}

function openSelectedTerminal() {
  openServerTab(selected.value);
}

function openPicked(index) {
  openServerTab(servers.value[index]);
}

async function copyEndpoint() {
  if (!selected.value) return;
  await navigator.clipboard.writeText(selected.value.endpoint);
  setStatus("Endpoint ID 已复制");
}

function focusActive() {
  const pane = activePane();
  if (pane) focusTerminal(pane.termName);
}

function closeActivePane() {
  if (activeTab.value) closePane(activeTab.value, activePane());
}

function closeActiveTab() {
  if (activeTab.value) closeTab(activeTab.value);
}

async function ctrl(sequence) {
  const pane = activePane();
  if (pane) await sendControl(pane, sequence);
}

watch(pickerOpen, (open) => {
  if (!open) splitMode.value = null;
});

watch(activeTab, async () => {
  await nextTick();
  focusActive();
});

function onKeydown(event) {
  if (!event.ctrlKey) return;
  const key = event.key.toLowerCase();
  if (event.shiftKey && key === "e") {
    event.preventDefault();
    requestSplit("row");
  } else if (event.shiftKey && key === "o") {
    event.preventDefault();
    requestSplit("col");
  } else if (event.shiftKey && key === "t") {
    event.preventDefault();
    splitMode.value = null;
    pickerOpen.value = true;
  } else if (event.shiftKey && key === "w") {
    event.preventDefault();
    closeActivePane();
  } else if (key === "tab") {
    event.preventDefault();
    cycleTab(event.shiftKey ? -1 : 1);
  } else if (/^[1-9]$/.test(key)) {
    const index = Number(key) - 1;
    if (tabs[index]) {
      event.preventDefault();
      activeTab.value = tabs[index];
    }
  }
}

function onClosePaneEvent(event) {
  const tab = tabs.find((item) => item.panes.some((pane) => pane.id === event.detail));
  const pane = tab?.panes.find((item) => item.id === event.detail);
  if (tab && pane) closePane(tab, pane);
}

window.addEventListener("error", (event) => {
  uiLog(`window error: ${event.message}`);
});
window.addEventListener("zero:ui-error", (event) => {
  setStatus(`界面出错：${event.detail}`);
});
window.addEventListener("unhandledrejection", (event) => {
  uiLog(`unhandled rejection: ${event.reason}`);
});

onMounted(async () => {
  uiLog("app mounted");
  await initSessionEvents();
  await loadServers();
  try {
    const auto = await invoke("auto_endpoint");
    if (auto) {
      uiLog(`auto open ${auto.slice(0, 24)}…`);
      openServerTab({ name: "auto", endpoint: auto });
    }
  } catch {
    // 调试功能失败无所谓
  }
  window.addEventListener("keydown", onKeydown);
  window.addEventListener("zero:close-pane", onClosePaneEvent);
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown);
  window.removeEventListener("zero:close-pane", onClosePaneEvent);
});
</script>

<template>
  <n-config-provider :theme-overrides="themeOverrides" abstract>
    <div class="app">
      <header class="topbar">
        <div class="brand">
          <span class="brand-mark">Z</span>
          <div class="brand-text">
            <strong>ZERO</strong>
            <small>remote shell over iroh</small>
          </div>
        </div>
      </header>

      <div class="body">
        <aside class="sidebar">
          <div class="sidebar-head">
            <div class="sidebar-title">
              <span>服务器</span>
              <small>{{ servers.length }} 个已保存</small>
            </div>
            <n-button size="tiny" secondary @click="openCreate">添加</n-button>
          </div>
          <n-input v-model:value="search" clearable placeholder="搜索名称 / 标签" size="small" />
          <div class="server-list">
            <button
              v-for="item in filteredServers"
              :key="item.index"
              class="server-item"
              :class="{ active: item.index === selectedIndex, running: tabs.some((t) => t.endpoint === item.server.endpoint) }"
              @click="selectServer(item.index)"
              @dblclick="openServerTab(item.server)"
            >
              <span class="dot"></span>
              <span class="server-text">
                <span class="name">{{ item.server.name }}</span>
                <span v-if="item.server.tags?.length" class="tags-inline">
                  {{ item.server.tags.join(" · ") }}
                </span>
              </span>
            </button>
            <div v-if="!filteredServers.length" class="empty">还没有服务器</div>
          </div>
          <div class="sidebar-foot">
            <span>双击服务器直接开终端</span>
          </div>
        </aside>

        <main class="content">
          <!-- 终端工作区 -->
          <div v-if="tabs.length" class="tabbar">
              <div class="tab-scroll">
                <button
                  v-for="tab in tabs"
                  :key="tab.id"
                  class="term-tab"
                  :class="{ active: activeTab?.id === tab.id }"
                  @click="activeTab = tab"
                >
                  <span class="tab-dot"></span>
                  <span class="tab-title">{{ tab.title }}</span>
                  <span v-if="tab.panes.length > 1" class="tab-count">{{ tab.panes.length }}</span>
                  <span class="tab-close" @click.stop="closeTab(tab)">×</span>
                </button>
              </div>
              <n-dropdown trigger="click" :options="plusOptions" @select="onPlusSelect">
                <button class="tab-add" title="新建标签页（Ctrl+Shift+T）">＋</button>
              </n-dropdown>
          </div>

          <div v-show="activeTab" class="term-view">
            <div class="term-toolbar">
              <button class="ghost" @click="ctrl('\u0003')">Ctrl-C</button>
              <button class="ghost" @click="ctrl('\u001b')">Esc</button>
              <button class="ghost" @click="ctrl('\t')">Tab</button>
              <button class="ghost" @click="ctrl('\u001b[A')">↑</button>
              <button class="ghost" @click="ctrl('\u001b[B')">↓</button>
              <span class="divider-v"></span>
              <button class="ghost" @click="requestSplit('row')">⇋ 左右拆分</button>
              <button class="ghost" @click="requestSplit('col')">⇵ 上下拆分</button>
              <span class="spacer"></span>
              <span class="term-hint">Ctrl+Shift+T 新标签 · Ctrl+Shift+W 关窗格 · Ctrl+Shift+E/O 拆分</span>
              <button class="ghost danger" @click="closeActivePane">关闭窗格</button>
            </div>

            <div class="term-body-outer">
              <div
                v-for="tab in tabs"
                v-show="activeTab?.id === tab.id"
                :key="tab.id"
                class="term-body"
                :class="`dir-${tab.direction}`"
              >
                <div
                  v-for="pane in tab.panes"
                  :key="pane.termName"
                  class="term-pane"
                  :class="{ focused: tab.panes.length > 1 && tab.activePaneId === pane.id }"
                  :data-pane="pane.termName"
                  @click="setActivePane(tab, pane)"
                >
                  <div v-if="tab.panes.length > 1" class="pane-badge">{{ pane.serverName }}</div>
                  <XTermPane
                    :pane="pane"
                    :active="activeTab?.id === tab.id && tab.activePaneId === pane.id"
                  />
                </div>
              </div>
            </div>
          </div>

          <!-- 服务器详情 -->
          <template v-if="!activeTab">
            <div v-if="selected" class="content-scroll">
              <div class="detail-header">
                <div>
                  <p class="eyebrow">SERVER DETAILS</p>
                  <h2>{{ selected.name }}</h2>
                </div>
                <n-button type="primary" :loading="busy" @click="openSelectedTerminal">
                  打开终端
                </n-button>
              </div>

              <n-card title="Endpoint / Ticket" size="small" class="card">
                <div class="endpoint-row">
                  <code>{{ selected.endpoint }}</code>
                  <n-button size="small" @click="copyEndpoint">复制</n-button>
                </div>
              </n-card>

              <n-card title="标签" size="small" class="card">
                <div v-if="selected.tags?.length" class="tags">
                  <n-tag v-for="tag in selected.tags" :key="tag" size="small" type="info">{{ tag }}</n-tag>
                </div>
                <span v-else class="muted">无标签</span>
              </n-card>

              <n-card title="最近打开" size="small" class="card">
                <span>{{ selected.last_opened ? new Date(selected.last_opened * 1000).toLocaleString() : "从未打开" }}</span>
              </n-card>

              <div class="actions">
                <n-button @click="openEdit">编辑服务器</n-button>
                <n-button type="error" secondary @click="deleteTarget = selectedIndex; deleteOpen = true">删除服务器</n-button>
              </div>
            </div>
            <div v-else class="welcome">
            <div class="welcome-card">
              <div class="welcome-icon">⌘</div>
              <h3>还没有打开终端</h3>
              <p>从左侧选择服务器，双击或点「打开终端」开始；也可以直接点「新建标签页」。</p>
              <n-button type="primary" @click="pickerOpen = true">新建标签页</n-button>
            </div>
            </div>
          </template>

          <footer v-if="status" class="statusbar">{{ status }}</footer>
        </main>
      </div>
    </div>

    <n-modal v-model:show="modalOpen" preset="card" title="服务器信息" style="width: 440px">
      <n-form label-placement="top">
        <n-form-item label="名称">
          <n-input v-model:value="draft.name" placeholder="例如：开发机" />
        </n-form-item>
        <n-form-item label="Endpoint ID">
          <n-input v-model:value="draft.endpoint" placeholder="endpoint id 或 ticket" />
        </n-form-item>
        <n-form-item label="标签（逗号分隔）">
          <n-input v-model:value="draft.tags" placeholder="开发, Linux, 生产" />
        </n-form-item>
        <div class="modal-actions">
          <n-button @click="modalOpen = false">取消</n-button>
          <n-button type="primary" @click="saveServer">保存</n-button>
        </div>
      </n-form>
    </n-modal>

    <n-modal v-model:show="pickerOpen" preset="card" :title="splitMode ? '拆分到…' : '打开终端'" style="width: 440px">
      <div v-if="!servers.length" class="picker-empty">
        还没有服务器，先添加一个。
        <n-button size="small" type="primary" @click="pickerOpen = false; openCreate()">添加服务器</n-button>
      </div>
      <div v-else class="picker-list">
        <button
          v-for="item in filteredServers"
          :key="item.index"
          class="picker-item"
          :disabled="busy"
          @click="openPicked(item.index)"
        >
          <span class="dot"></span>
          <span class="name">{{ item.server.name }}</span>
          <span class="picker-endpoint">{{ item.server.endpoint.slice(0, 12) }}…</span>
        </button>
      </div>
    </n-modal>

    <n-modal v-model:show="deleteOpen" preset="dialog" type="warning" title="确认删除" content="确定删除当前服务器吗？">
      <template #action>
        <div class="modal-actions">
          <n-button @click="deleteOpen = false; deleteTarget = null">取消</n-button>
          <n-button type="error" @click="removeServer">删除</n-button>
        </div>
      </template>
    </n-modal>
  </n-config-provider>
</template>

<style>
:root {
  --bg: #0b0f14;
  --surface: #11161d;
  --surface-2: #151c26;
  --elevated: #1a2230;
  --border: #1e2836;
  --border-soft: #19222e;
  --text: #e6edf5;
  --text-2: #c3ccd8;
  --text-3: #8794a5;
  --text-dim: #5d6b7d;
  --accent: #5eead4;
  --accent-2: #7dd3fc;
  --green: #6ee7a8;
  --red: #f87171;
  --yellow: #fbbf24;
}

* { box-sizing: border-box; }

html, body, #app {
  height: 100%;
  margin: 0;
}

body {
  font-family: Inter, "Microsoft YaHei", "Noto Sans CJK SC", sans-serif;
  color: var(--text);
  background: var(--bg);
}

.app {
  display: flex;
  flex-direction: column;
  height: 100%;
}

/* topbar */
.topbar {
  flex: 0 0 auto;
  height: 56px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  padding: 0 18px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
}

.brand {
  display: flex;
  align-items: center;
  gap: 10px;
}

.brand-mark {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-weight: 800;
  color: #06221f;
  background: linear-gradient(135deg, #5eead4, #38bdf8);
}

.brand-text {
  display: flex;
  flex-direction: column;
  line-height: 1.1;
}

.brand-text strong {
  letter-spacing: .12em;
  font-size: 14px;
  color: var(--text);
}

.brand-text small {
  font-size: 10px;
  letter-spacing: .08em;
  color: var(--text-dim);
  text-transform: uppercase;
}

/* main area */
.body {
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
}

/* sidebar */
.sidebar {
  flex: 0 0 252px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-height: 0;
  padding: 14px 12px 10px;
  background: var(--surface);
  border-right: 1px solid var(--border);
}

.sidebar-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 2px;
}

.sidebar-title span {
  display: block;
  font-size: 14px;
  font-weight: 700;
  color: var(--text);
}

.sidebar-title small {
  color: var(--text-3);
  font-size: 11px;
}

.server-list {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding-right: 2px;
}

.server-item {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  padding: 8px 10px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: transparent;
  color: var(--text-2);
  font-size: 13px;
  cursor: pointer;
  text-align: left;
  transition: background .12s, border-color .12s, color .12s;
}

.server-item:hover {
  background: var(--surface-2);
  color: var(--text);
}

.server-item.active {
  background: rgba(94, 234, 212, .09);
  border-color: rgba(94, 234, 212, .28);
  color: var(--accent);
}

.server-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.tags-inline {
  font-size: 10.5px;
  color: var(--text-dim);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dot {
  flex: 0 0 auto;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--text-dim);
  transition: background .12s;
}

.server-item:hover .dot {
  background: var(--text-3);
}

.server-item.active .dot {
  background: var(--accent);
  box-shadow: 0 0 6px rgba(94, 234, 212, .55);
}

.server-item.running .dot {
  background: var(--green);
  box-shadow: 0 0 6px rgba(110, 231, 168, .6);
}

.name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.empty {
  padding: 16px 8px;
  color: var(--text-3);
  font-size: 12px;
  text-align: center;
}

.sidebar-foot {
  padding: 6px 2px 0;
  border-top: 1px solid var(--border-soft);
  color: var(--text-dim);
  font-size: 10.5px;
}

/* content */
.content {
  flex: 1 1 auto;
  display: flex;
  flex-direction: column;
  min-width: 0;
  background: var(--bg);
}

.content-scroll {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  padding: 24px 30px 30px;
}

.detail-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  padding-bottom: 18px;
  border-bottom: 1px solid var(--border-soft);
}

.eyebrow {
  margin: 0 0 6px;
  color: var(--accent-2);
  font-size: 10.5px;
  letter-spacing: .14em;
}

.detail-header h2 {
  margin: 0;
  font-size: 24px;
  color: var(--text);
}

.card {
  margin-top: 14px;
}

.endpoint-row {
  display: flex;
  align-items: center;
  gap: 12px;
}

.endpoint-row code {
  flex: 1;
  min-width: 0;
  font-family: "JetBrains Mono", ui-monospace, monospace;
  font-size: 12.5px;
  overflow-wrap: anywhere;
  color: var(--text-2);
}

.tags {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.muted {
  color: var(--text-3);
}

.actions {
  display: flex;
  gap: 10px;
  margin-top: 18px;
}

.welcome {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  padding: 24px;
}

.welcome-card {
  max-width: 380px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
}

.welcome-icon {
  width: 46px;
  height: 46px;
  border-radius: 14px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 20px;
  color: var(--accent);
  background: rgba(94, 234, 212, .1);
  border: 1px solid rgba(94, 234, 212, .22);
}

.welcome-card h3 {
  margin: 4px 0 0;
  font-size: 17px;
}

.welcome-card p {
  margin: 0 0 8px;
  color: var(--text-3);
  font-size: 12.5px;
  line-height: 1.6;
}

/* 终端工作区 */
.tabbar {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
}

.tab-scroll {
  display: flex;
  gap: 4px;
  overflow-x: auto;
  scrollbar-width: none;
}

.tab-scroll::-webkit-scrollbar {
  display: none;
}

.term-tab {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 6px 10px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: transparent;
  color: var(--text-3);
  font-size: 12.5px;
  cursor: pointer;
  white-space: nowrap;
}

.term-tab:hover {
  background: var(--surface-2);
  color: var(--text-2);
}

.term-tab.active {
  background: var(--surface-2);
  border-color: var(--border);
  color: var(--text);
}

.term-tab.closed .tab-dot {
  background: var(--red);
}

.term-tab .tab-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--green);
  box-shadow: 0 0 5px rgba(110, 231, 168, .7);
}

.tab-title {
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tab-close {
  font-size: 14px;
  line-height: 1;
  color: var(--text-dim);
  padding: 0 2px;
  border-radius: 4px;
}

.tab-close:hover {
  color: var(--red);
  background: rgba(248, 113, 113, .12);
}

.tab-add {
  flex: 0 0 auto;
  width: 26px;
  height: 26px;
  border-radius: 7px;
  border: 1px dashed var(--border);
  background: transparent;
  color: var(--text-3);
  font-size: 15px;
  cursor: pointer;
}

.tab-add:hover {
  color: var(--accent);
  border-color: rgba(94, 234, 212, .5);
}

.term-toolbar {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 10px;
  background: var(--bg);
  border-bottom: 1px solid var(--border-soft);
}

.term-toolbar .ghost {
  border: 1px solid var(--border);
  background: transparent;
  color: var(--text-3);
  border-radius: 6px;
  padding: 3px 8px;
  font-size: 11.5px;
  cursor: pointer;
}

.term-toolbar .ghost:hover {
  color: var(--text);
  border-color: var(--text-dim);
}

.term-toolbar .ghost.danger:hover {
  color: var(--red);
  border-color: rgba(248, 113, 113, .5);
}

.term-toolbar .spacer {
  flex: 1;
}

.term-hint {
  font-size: 11px;
  color: var(--text-dim);
}

.term-view {
  flex: 1 1 auto;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.term-body-outer {
  flex: 1 1 auto;
  min-height: 0;
  display: flex;
}

.term-body {
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
  display: flex;
  gap: 8px;
  padding: 10px;
}

.term-body.dir-row {
  flex-direction: row;
}

.term-body.dir-col {
  flex-direction: column;
}

.term-pane {
  flex: 1 1 0;
  min-width: 0;
  min-height: 0;
  position: relative;
  border: 1px solid transparent;
  border-radius: 10px;
}

.term-pane.focused {
  border-color: rgba(94, 234, 212, .3);
}

.pane-badge {
  position: absolute;
  top: 6px;
  right: 10px;
  z-index: 3;
  padding: 2px 6px;
  border-radius: 5px;
  font-size: 10px;
  color: var(--text-dim);
  background: rgba(11, 15, 20, .78);
}

.tab-count {
  font-size: 10px;
  line-height: 1;
  padding: 2px 4px;
  border-radius: 4px;
  border: 1px solid var(--border);
  color: var(--text-dim);
}

.divider-v {
  width: 1px;
  height: 16px;
  margin: 0 4px;
  background: var(--border);
}

/* xterm 窗格 */
.term-pane .xterm-host {
  border-radius: 10px;
  overflow: hidden;
}

.term-pane .xterm {
  padding: 2px 0;
}

.statusbar {
  flex: 0 0 auto;
  padding: 8px 18px;
  border-top: 1px solid var(--border-soft);
  color: var(--text-3);
  font-size: 11.5px;
  background: var(--surface);
}

.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.picker-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 320px;
  overflow-y: auto;
}

.picker-item {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 9px 10px;
  border: 1px solid var(--border-soft);
  border-radius: 8px;
  background: transparent;
  color: var(--text-2);
  font-size: 13px;
  cursor: pointer;
  text-align: left;
}

.picker-item:hover {
  border-color: rgba(94, 234, 212, .35);
  background: rgba(94, 234, 212, .06);
  color: var(--text);
}

.picker-endpoint {
  margin-left: auto;
  font-family: "JetBrains Mono", ui-monospace, monospace;
  font-size: 10.5px;
  color: var(--text-dim);
}

.picker-empty {
  display: flex;
  flex-direction: column;
  gap: 10px;
  align-items: flex-start;
  color: var(--text-3);
  font-size: 13px;
}
</style>
