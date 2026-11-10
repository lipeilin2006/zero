# zero-client

Zero 的图形客户端：管理保存的服务器，并内置多标签 / 多窗格终端（仿 Windows Terminal 的操作模型）。

## 功能

- 服务器列表：添加 / 编辑 / 删除 / 搜索，标签，记住最近打开
- 标签栏第一个永远是「服务器」标签（不可关闭），后面是终端标签，＋ 一直在右侧
- 多标签：每个终端标签一条到 `zero-server` 的连接
- 多窗格：一个标签内可以左右或上下拆分，每个窗格一条独立会话
- 终端用 xterm.js：完整 ANSI 终端仿真，vim / top / htop 都能用
- 快捷键（和 Windows Terminal 对齐）：
  - `Ctrl+Shift+T` 新建标签
  - `Ctrl+Shift+W` 关闭窗格
  - `Ctrl+Tab` / `Ctrl+Shift+Tab` 切换标签
  - `Ctrl+1` 服务器标签，`Ctrl+2..9` 第 N-1 个终端标签
  - `Ctrl+Shift+E` 左右拆分，`Ctrl+Shift+O` 上下拆分
  - 其余按键（Ctrl+C、Ctrl+D、Ctrl+Z…）直接交给远端 shell
- 终端工具栏：Ctrl-C、Esc、Tab、上下键、拆分、关闭窗格
- 工具栏：Ctrl-C、Esc、Tab、拆分、关闭窗格（这些键现在也可以直接敲）

## 目录

| 文件 | 作用 |
|---|---|
| `src-tauri/src/session.rs` | 会话管理：复用 CLI 的客户端密钥，直接走 `zero-proto` 连接服务器 |
| `src-tauri/src/lib.rs` | Tauri 命令：`session_open` / `session_input` / `session_resize` / `session_close` |
| `src/terminals.js` | 标签 / 窗格状态、会话事件、xterm 实例注册 |
| `src/components/XTermPane.vue` | 一个 xterm 窗格：FitAddon 自适应、onData 转发、onResize 同步 |
| `src/App.vue` | 界面：服务器管理 + 标签栏 + 窗格布局 |

服务器列表和 CLI 共用一份文件：`$DATA/zero-client/servers.json`。

## 运行

```bash
npm install --include=dev
npm run tauri dev
```

> 有些环境的 npm 默认 `omit=dev`，会装不上 vite，所以显式带 `--include=dev`。

服务端：

```bash
zero-server                        # 默认启动，走 authorized_keys 白名单
zero-server --locale C.UTF-8       # 强制 PTY 用 UTF-8（默认也是自动挑 UTF-8）
zero-server --allow-all            # 调试用：谁都能连
zero-server trust <client-id>      # 信任一个客户端，运行时自动生效
zero-server trusted                # 看白名单
zero-server untrust <client-id>    # 取消信任
```

客户端 id 用 `zero --show-id` 取。

服务端启动日志里还有一行 `server ticket: ...`（带地址的 ticket）。
客户端「Endpoint ID」那一栏既接受裸 endpoint id，也接受这个 ticket；
ticket 不依赖发现服务，刚启动的服务器用 ticket 能立刻连上。
也可以单独打印：`zero-server ticket`。

CLI 也可以管理服务器列表（和 GUI 共享）：

```bash
zero add dev <endpoint> --tags linux,dev
zero list
zero show dev
zero rm dev
zero dev                           # 直接按名字连接
```

## 调试

两个环境变量，只在开发/排查时用：

- `ZERO_UI_LOG=/tmp/ui.log`：前端把关键步骤（打开会话、xterm 挂载、首个输出/输入）追加到文件
- `ZERO_AUTO_ENDPOINT=<ticket 或 endpoint id>`：启动后自动打开一个终端，方便无头验证

配合 `xvfb-run` 可以在没有显示器的机器上跑起来截图。

## 已知限制

- 拆分是等分，暂不支持拖拽调整比例
- 标签栏暂不支持拖动排序
- 没有复制/粘贴的右键菜单（浏览器/系统默认快捷键可用）
