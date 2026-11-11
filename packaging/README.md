# 部署

## 一键安装（systemd）

```bash
# 自动下载当前架构的 release 二进制
curl -fsSL https://raw.githubusercontent.com/lipeilin2006/zero/main/install-server.sh \
  | sudo bash

# 或者用本地二进制
sudo ./install-server.sh --bin ./target/release/zero-server
```

做了什么：

1. 创建专用系统用户 `zero`（已存在则复用）
2. 安装二进制到 `/usr/local/bin/zero-server`
3. 配置目录 `/etc/zero/server`：`config.json`、`secret.key`、`authorized_keys`
4. 写 `/etc/systemd/system/zero-server.service` 并 `enable --now`
5. 打印服务端 endpoint id

## 配置文件

默认 `/etc/zero/server/config.json`：

```json
{
  "key": "/etc/zero/server/secret.key",
  "allowlist": "/etc/zero/server/authorized_keys",
  "shell": null,
  "locale": null,
  "allow_all": false
}
```

默认 unit 允许远端 `su`/`sudo`；加 `--hardened` 会启用严格沙箱（禁止 su/sudo）。

- `shell` 不填时按优先级选：Linux/macOS `fish > zsh > bash > sh`；Windows `pwsh > powershell > cmd`
- `locale` 不填时用服务端 UTF-8 locale，否则 `C.UTF-8`
- 路径不填时取配置文件同目录下的默认文件
- 别的配置文件：`zero-server -c /path/config.json run`

## 管理命令

全局安装后不用带 `-c`：

```bash
zero-server id
zero-server trust <client-id>
zero-server trusted
zero-server untrust <client-id>
```

客户端 id 用 `zero --show-id` 取；服务运行中改 allowlist 会自动生效。

## 卸载

```bash
sudo ./install-server.sh --uninstall          # 保留 /etc/zero/server
sudo ./install-server.sh --uninstall --purge  # 连配置目录和 zero 用户一起删
```

## 参数

| 参数 | 说明 |
|---|---|
| `--bin <path>` | 指定本地二进制 |
| `--config-dir <path>` | 配置目录，默认 `/etc/zero/server` |
| `--user <name>` | 运行用户，默认 `zero` |
| `--arch <arch>` | 覆盖架构检测（x86_64 / aarch64） |
| `--version <tag>` | 下载指定版本，默认 latest |
| `--allow-any` | 调试用，跳过白名单 |
| `--no-download` | 只用本地二进制 |
| `--dry-run` | 只打印命令和 unit |

## 手动安装

参考 `packaging/zero-server.service`（默认 zero 用户 + `/etc/zero/server` 的写法）。
