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

脚本会自动：创建系统用户、安装二进制、生成密钥、写 systemd unit、启动服务。

```bash
# 一行安装（自动下载 release 里的服务端二进制）
curl -fsSL https://raw.githubusercontent.com/lipeilin2006/zero/main/packaging/install-server.sh \
  | sudo bash -s -- --download

# 或者在仓库根目录，用本地构建好的二进制
sudo packaging/install-server.sh --bin ./target/release/zero-server

# 调试：跳过白名单
sudo packaging/install-server.sh --allow-any
```

安装完成后脚本会打印服务器 endpoint id。把客户端的 id 加进白名单：

```bash
# 客户端取 id
zero --show-id

# 服务端信任它（服务运行中也会自动生效）
sudo -u zero /usr/local/bin/zero-server trust <client-id> \
  --key /var/lib/zero-server/secret.key \
  --allowlist /var/lib/zero-server/authorized_keys
```

取带地址的 ticket（客户端填这个比裸 id 更稳，不依赖发现服务）：

```bash
sudo -u zero /usr/local/bin/zero-server ticket \
  --key /var/lib/zero-server/secret.key
```

卸载：`sudo packaging/install-server.sh --uninstall`（加 `--purge` 连数据目录一起删）。

## 客户端

图形客户端：服务器列表是第一个标签页（不可关闭），`＋` 新建终端标签，支持左右/上下拆分、xterm.js 完整终端。

命令行：

```bash
zero --show-id                 # 打印本机客户端 id
zero add dev <ticket|id>       # 保存服务器
zero list                      # 列表
zero dev                       # 按名字连接
zero rm dev                    # 删除
```

## 发布

打 tag 会触发 GitHub Actions：

- `v0.1.0` 这类 tag → 构建服务端二进制包 + 各平台客户端安装包，自动创建 GitHub Release
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
