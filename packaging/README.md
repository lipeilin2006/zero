# 部署

## 发布产物里的服务端包按架构分开：`zero-server-<tag>-x86_64-linux.tar.gz` / `...-aarch64-linux.tar.gz`，脚本会自动识别架构并下载对应的包。

## 一键安装（systemd）

```bash
# 一行安装：自动从 GitHub Release 下载 x86_64 二进制
curl -fsSL https://raw.githubusercontent.com/lipeilin2006/zero/main/install-server.sh \
  | sudo bash -s -- --download

# 或者用本地二进制
sudo ./install-server.sh --bin ./target/release/zero-server
```

会做这些事：

1. 以执行 `sudo` 的用户身份运行（`--user` 可覆盖；默认不创建新用户）
2. 建数据目录 `~/.local/share/zero-server`（0750，属于该用户）
3. 安装二进制到 `/usr/local/bin/zero-server`
4. 生成 `secret.key`（0600）和空的 `authorized_keys`（0600）
5. 写 `/etc/systemd/system/zero-server.service` 并 `enable --now`
6. 打印服务器 endpoint id 和后续操作提示

> 远端 shell 就是该用户的 shell：`id -un` 会显示安装时执行 sudo 的用户，
> 工作目录是该用户的 home。用 `--user zero` 才会换成专用用户。

常用参数：

| 参数 | 说明 |
|---|---|
| `--bin <path>` | 指定本地二进制（默认找 `./zero-server` 或 `target/release/zero-server`） |
| `--user <name>` | 运行用户，默认执行 sudo 的当前用户 |
| `--data-dir <path>` | 数据目录，默认 `~/.local/share/zero-server` |
| `--allow-any` | 调试用，跳过白名单 |
| `--uninstall` | 停止并删除服务和二进制（保留数据） |
| `--purge` | 配合 `--uninstall`，连数据目录一起删 |
| `--download` | 没有本地二进制时从 Release 下载（支持 x86_64 Linux） |
| `--version <tag>` | 配合 `--download` 指定版本，默认 latest |
| `--dry-run` | 只打印要执行的命令和 unit 内容 |

## 信任客户端

```bash
# 客户端上
zero --show-id

# 服务端上（运行中也生效，不用重启）
/usr/local/bin/zero-server trust <client-id> \
  --key ~/.local/share/zero-server/secret.key \
  --allowlist ~/.local/share/zero-server/authorized_keys

# 查看 / 取消
/usr/local/bin/zero-server trusted --key ~/.local/share/zero-server/secret.key
/usr/local/bin/zero-server untrust <client-id> --key ~/.local/share/zero-server/secret.key
```

## 给客户端填什么

推荐用带地址的 ticket，不依赖发现服务的传播：

```bash
cat ~/.local/share/zero-server/ticket
```

> 服务端启动时会把 ticket 写到数据目录；`zero-server ticket` 也是读这个文件。
> 不要用 `ticket --bind` 对着正在运行的服务生成——那会顶掉它的 relay 连接。

也可以填裸 endpoint id（`zero-server id`），客户端会自动重试等发现记录。

## 手动安装

参考 `packaging/zero-server.service`（那是「专用用户 zero + /var/lib」的写法），
也可以直接用一键脚本 `--user zero`、`--data-dir /var/lib/zero-server` 生成同样的配置。

```bash
systemctl daemon-reload
systemctl enable --now zero-server
```
