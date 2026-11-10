# zero

基于 [iroh](https://iroh.computer) 的远程 shell 工具。思路和 SSH 类似，但没有公网 IP、端口转发这些麻烦事：两端各自生成一个 endpoint id，连上就能开一个远端 PTY，客户端可以是命令行，也可以是图形界面。

```
┌──────────────┐        iroh (QUIC, 打洞/中继)        ┌──────────────┐
│ zero / GUI   │  ───────────────────────────────▶  │ zero-server  │
│  (客户端)     │        zero-proto 协议              │  (PTY + shell)│
└──────────────┘                                     └──────────────┘
```

## 组成

| 目录 | 说明 |
|---|---|
| `zero-proto` | 协议：消息定义 + 长度前缀分帧 |
| `zero-store` | 服务器列表（CLI 和图形客户端共用一份 JSON） |
| `zero-server` | 服务端：接受连接 → 校验 allowlist → 起 PTY → 转发字节 |
| `zero` | 命令行客户端 |
| `zero-client/` | Tauri 图形客户端（Vue 3 + xterm.js，多标签/多窗格） |
| `packaging/` | 一键部署脚本、systemd unit |
| `docs/REVIEW.md` | 代码审查：已修问题、已知问题和安全模型 |

服务端和客户端**分开构建、分开发布**：服务端是单个静态二进制，客户端是各平台安装包。

服务端启动远端 shell 的优先级：

- Linux/macOS：`fish` > `zsh` > `bash` > `sh`
- Windows：`pwsh` > `powershell` > `cmd`

可以用 `zero-server run --shell <path>` 覆盖。

## 快速开始（本机）

```bash
# 服务端
cargo run -p zero-server -- --allow-all

# 另开一个终端，用上面的 server endpoint id 连接
cargo run -p zero -- <server-endpoint-id>
```

图形客户端：

```bash
cd zero-client
npm install --include=dev
npm run tauri dev
```

## 部署服务端（一键）

脚本做全局安装：二进制到 `/usr/local/bin`，配置和密钥统一放 `/etc/zero/server`，
创建专用 `zero` 用户和 systemd 服务。

```bash
# 一行安装（自动下载当前架构的服务端）
curl -fsSL https://raw.githubusercontent.com/lipeilin2006/zero/main/install-server.sh \
  | sudo bash

# 或者用本地构建的二进制
sudo ./install-server.sh --bin ./target/release/zero-server

# 调试：跳过白名单
sudo ./install-server.sh --allow-any
```

### 配置文件

Linux 默认 `/etc/zero/server/config.json`：

```json
{
  "key": "/etc/zero/server/secret.key",
  "allowlist": "/etc/zero/server/authorized_keys",
  "shell": null,
  "locale": null,
  "allow_all": false
}
```

- 文件里没写的路径会用配置文件同目录下的默认值（`secret.key`、`authorized_keys`）
- 改完配置：`sudo systemctl restart zero-server`
- 临时换一份配置：`zero-server -c /path/to/config.json <命令>`
- 其它平台默认位置：macOS `/Library/Application Support/zero/server/config.json`，
  Windows `%ProgramData%\zero\server\config.json`

### 管理命令

全局安装后不用带 `-c`：

```bash
zero-server id                    # 服务端 id
zero-server trust <client-id>     # 信任客户端（运行中自动生效）
zero-server trusted               # 查看白名单
zero-server untrust <client-id>   # 取消信任
```

客户端 id 用 `zero --show-id` 取。

卸载：`sudo ./install-server.sh --uninstall`（加 `--purge` 连配置目录和用户一起删）。

## 客户端

图形客户端：服务器列表是第一个标签页（不可关闭），`＋` 新建终端标签，支持左右/上下拆分、xterm.js 完整终端。

命令行：

```bash
zero --show-id                 # 打印本机客户端 id
zero add dev <id>              # 保存服务器（endpoint id）
zero list                      # 列表
zero dev                       # 按名字连接
zero rm dev                    # 删除
```

## 发布

打 tag 会触发 GitHub Actions：

- `v0.1.x` 这类 tag → 构建服务端二进制包（x86_64 / aarch64）和各平台客户端安装包，自动创建 GitHub Release
- 服务端包名：`zero-server-<tag>-<arch>-linux.tar.gz`；客户端按平台/架构分别出包
- 普通 push / PR → CI（fmt、clippy、测试、前端构建）

```bash
git tag v0.1.0
git push origin v0.1.0
```

## 安全

- 认证 = iroh 端点身份 + 服务端 `authorized_keys` 白名单（按客户端公钥）
- 私钥文件 0600，服务端 systemd 沙箱（`ProtectSystem=strict`、无 capabilities）
- 只提供 shell，没有端口转发、没有文件传输
- `--allow-all` 只适合本机调试

更多见 [docs/REVIEW.md](docs/REVIEW.md)。

## License

MIT，见 [LICENSE](LICENSE)。
