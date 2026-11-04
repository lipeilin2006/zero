<script setup>
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

const servers = ref([]);
const selectedIndex = ref(null);
const search = ref("");
const modalOpen = ref(false);
const draft = ref({ name: "", endpoint: "", tags: "" });
const deleteTarget = ref(null);
const deleteOpen = ref(false);
const status = ref("");

const themeOverrides = {
  common: {
    fontFamily: 'Inter, "Microsoft YaHei", "Noto Sans CJK SC", sans-serif',
    bodyColor: "#2e3440",
    cardColor: "#3b4252",
    modalColor: "#3b4252",
    popoverColor: "#3b4252",
    tableColor: "#3b4252",
    actionBarColor: "#3b4252",
    inputColor: "#434c5e",
    actionColor: "#434c5e",
    textColorBase: "#eceff4",
    textColor1: "#eceff4",
    textColor2: "#d8dee9",
    textColor3: "#8490a0",
    placeholderColor: "#616e88",
    borderColor: "#4c566a",
    dividerColor: "#4c566a",
    primaryColor: "#88c0d0",
    primaryColorHover: "#8fbcbb",
    primaryColorPressed: "#5e81ac",
    primaryColorSuppl: "#81a1c1",
    infoColor: "#81a1c1",
    infoColorHover: "#88c0d0",
    infoColorPressed: "#5e81ac",
    successColor: "#a3be8c",
    successColorHover: "#b0c99b",
    successColorPressed: "#8fa878",
    warningColor: "#ebcb8b",
    warningColorHover: "#f0d69c",
    warningColorPressed: "#d4b07a",
    errorColor: "#bf616a",
    errorColorHover: "#cc7380",
    errorColorPressed: "#a85560",
    borderRadius: "6px",
    borderRadiusSmall: "4px",
  },
  Button: {
    fontWeightStrong: "600",
    heightMedium: "32px",
    heightSmall: "26px",
    paddingMedium: "0 14px",
    paddingSmall: "0 10px",
  },
  Input: {
    color: "#434c5e",
    colorFocus: "#4c566a",
    colorDisabled: "#3b4252",
    border: "1px solid #4c566a",
    borderHover: "1px solid #81a1c1",
    borderFocus: "1px solid #88c0d0",
    boxShadowFocus: "0 0 0 2px rgba(136, 192, 208, .25)",
    placeholderColor: "#616e88",
  },
  Card: {
    color: "#3b4252",
    borderColor: "#4c566a",
    titleFontSizeMedium: "13px",
    titleFontWeight: "600",
  },
  Tag: {
    color: "#434c5e",
    colorBordered: "#434c5e",
    textColor: "#d8dee9",
    textColorBordered: "#d8dee9",
    borderColor: "#4c566a",
    borderRadius: "4px",
  },
  Scrollbar: {
    color: "rgba(136, 192, 208, .25)",
    colorHover: "rgba(136, 192, 208, .4)",
  },
  Dialog: {
    color: "#3b4252",
    borderColor: "#4c566a",
    titleFontSize: "15px",
    titleFontWeight: "600",
  },
  Popover: {
    color: "#434c5e",
  },
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

async function loadServers() {
  try {
    servers.value = await invoke("list_servers");
  } catch (error) {
    status.value = `加载失败：${error}`;
  }
}

function selectServer(index) {
  selectedIndex.value = index;
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
    status.value = "名称和 Endpoint ID 不能为空";
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
    status.value = `已保存：${name}`;
  } catch (error) {
    status.value = `保存失败：${error}`;
  }
}

async function removeServer() {
  if (deleteTarget.value === null) return;
  servers.value.splice(deleteTarget.value, 1);
  selectedIndex.value = null;
  deleteTarget.value = null;
  deleteOpen.value = false;
  await invoke("save_servers", { servers: servers.value });
  status.value = "服务器已删除";
}

async function openTerminal() {
  if (!selected.value) return;
  selected.value.last_opened = Math.floor(Date.now() / 1000);
  try {
    await invoke("save_servers", { servers: servers.value });
    await invoke("launch_zero", { endpoint: selected.value.endpoint });
    status.value = `已打开终端：${selected.value.name}`;
  } catch (error) {
    status.value = `启动失败：${error}`;
  }
}

async function copyEndpoint() {
  if (!selected.value) return;
  await navigator.clipboard.writeText(selected.value.endpoint);
  status.value = "Endpoint ID 已复制";
}

onMounted(loadServers);
</script>

<template>
  <n-config-provider :theme-overrides="themeOverrides" abstract>
    <div class="app">
      <header class="topbar">
        <div class="brand">ZERO <span>SERVER MANAGER</span></div>
        <span class="divider"></span>
        <span class="topbar-note">远程终端管理</span>
      </header>

      <div class="body">
        <aside class="sidebar">
          <div class="sidebar-head">
            <div class="sidebar-title">
              <span>服务器</span>
              <small>{{ servers.length }} 个已保存</small>
            </div>
            <n-button size="small" type="primary" @click="openCreate">添加</n-button>
          </div>
          <n-input v-model:value="search" clearable placeholder="搜索服务器" size="small" />
          <div class="server-list">
            <button
              v-for="item in filteredServers"
              :key="item.index"
              class="server-item"
              :class="{ active: item.index === selectedIndex }"
              @click="selectServer(item.index)"
              @dblclick="openTerminal"
            >
              <span class="dot"></span>
              <span class="name">{{ item.server.name }}</span>
            </button>
            <div v-if="!filteredServers.length" class="empty">没有匹配的服务器</div>
          </div>
        </aside>

        <main class="content">
          <div class="content-scroll">
            <template v-if="selected">
              <div class="detail-header">
                <div>
                  <p class="eyebrow">SERVER DETAILS</p>
                  <h2>{{ selected.name }}</h2>
                </div>
                <n-button type="primary" @click="openTerminal">打开终端</n-button>
              </div>

              <n-card title="Endpoint ID" size="small" class="card">
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
            </template>

            <div v-else class="welcome">
              <p>从左侧选择服务器，查看详情或打开终端</p>
            </div>
          </div>
          <footer v-if="status" class="statusbar">{{ status }}</footer>
        </main>
      </div>
    </div>

    <n-modal v-model:show="modalOpen" preset="card" title="服务器信息" style="width: 430px">
      <n-form label-placement="top">
        <n-form-item label="名称">
          <n-input v-model:value="draft.name" placeholder="例如：开发机" />
        </n-form-item>
        <n-form-item label="Endpoint ID">
          <n-input v-model:value="draft.endpoint" placeholder="6300de3372e3..." />
        </n-form-item>
        <n-form-item label="标签（逗号分隔）">
          <n-input v-model:value="draft.tags" placeholder="开发、Linux、生产" />
        </n-form-item>
        <div class="modal-actions">
          <n-button @click="modalOpen = false">取消</n-button>
          <n-button type="primary" @click="saveServer">保存</n-button>
        </div>
      </n-form>
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
  --bg: #2e3440;
  --surface: #3b4252;
  --elevated: #434c5e;
  --border: #4c566a;
  --border-soft: #434c5e;
  --text: #eceff4;
  --text-2: #d8dee9;
  --text-3: #8490a0;
  --text-dim: #616e88;
  --accent: #88c0d0;
  --accent-2: #81a1c1;
  --accent-3: #5e81ac;
  --green: #a3be8c;
  --red: #bf616a;
  --yellow: #ebcb8b;
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
  height: 52px;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 0 20px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
}

.brand {
  font-weight: 800;
  letter-spacing: .08em;
  color: var(--accent);
  white-space: nowrap;
}

.brand span {
  margin-left: 8px;
  font-size: 10px;
  color: var(--text-dim);
}

.divider {
  width: 1px;
  height: 18px;
  background: var(--border);
}

.topbar-note {
  color: var(--text-3);
  font-size: 13px;
}

/* main area */
.body {
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
}

/* sidebar */
.sidebar {
  flex: 0 0 250px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-height: 0;
  padding: 16px 12px;
  background: var(--surface);
  border-right: 1px solid var(--border);
}

.sidebar-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.sidebar-title span {
  display: block;
  font-size: 15px;
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
  border-radius: 6px;
  background: transparent;
  color: var(--text-2);
  font-size: 13px;
  cursor: pointer;
  text-align: left;
  transition: background .12s, border-color .12s, color .12s;
}

.server-item:hover {
  background: var(--elevated);
  color: var(--text);
}

.server-item.active {
  background: rgba(136, 192, 208, .13);
  border-color: rgba(136, 192, 208, .35);
  color: var(--accent);
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
  box-shadow: 0 0 6px rgba(136, 192, 208, .5);
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
  padding: 26px 32px 32px;
}

.detail-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  padding-bottom: 20px;
  border-bottom: 1px solid var(--border-soft);
}

.eyebrow {
  margin: 0 0 6px;
  color: var(--accent-2);
  font-size: 11px;
  letter-spacing: .12em;
}

.detail-header h2 {
  margin: 0;
  font-size: 26px;
  color: var(--text);
}

.card {
  margin-top: 16px;
}

.endpoint-row {
  display: flex;
  align-items: center;
  gap: 12px;
}

.endpoint-row code {
  flex: 1;
  min-width: 0;
  font-size: 13px;
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
  margin-top: 20px;
}

.welcome {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--text-3);
  font-size: 14px;
}

.statusbar {
  flex: 0 0 auto;
  padding: 9px 32px;
  border-top: 1px solid var(--border-soft);
  color: var(--text-3);
  font-size: 12px;
  background: var(--surface);
}

.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>
