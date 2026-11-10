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

1. 创建系统用户 `zero`（已存在则跳过）
2. 建数据目录 `/var/lib/zero-server`（0750）
3. 安装二进制到 `/usr/local/bin/zero-server`
4. 生成 `secret.key`（0600）和空的 `authorized_keys`（0600）
5. 写 `/etc/systemd/system/zero-server.service` 并 `enable --now`
6. 打印服务器 endpoint id 和后续操作提示

常用参数：

| 参数 | 说明 |
|---|---|
| `--bin <path>` | 指定本地二进制（默认找 `./zero-server` 或 `target/release/zero-server`） |
| `--user <name>` | 运行用户，默认 `zero` |
| `--data-dir <path>` | 数据目录，默认 `/var/lib/zero-server` |
| `--allow-any` | 调试用，跳过白名单 |
| `--uninstall` | 停止并删除服务和二进制（保留数据） |
| `--purge` | 配合 `--uninstall`，连数据目录和用户一起删 |
| `--download` | 没有本地二进制时从 Release 下载（支持 x86_64 Linux） |
| `--version <tag>` | 配合 `--download` 指定版本，默认 latest |
| `--dry-run` | 只打印要执行的命令和 unit 内容 |

## 信任客户端

```bash
# 客户端上
zero --show-id

# 服务端上（运行中也生效，不用重启）
sudo -u zero /usr/local/bin/zero-server trust <client-id> \
  --key /var/lib/zero-server/secret.key \
  --allowlist /var/lib/zero-server/authorized_keys

# 查看 / 取消
sudo -u zero /usr/local/bin/zero-server trusted --key /var/lib/zero-server/secret.key
sudo -u zero /usr/local/bin/zero-server untrust <client-id> --key /var/lib/zero-server/secret.key
```

## 给客户端填什么

推荐用带地址的 ticket，不依赖发现服务的传播：

```bash
cat /var/lib/zero-server/ticket
```

> 服务端启动时会把 ticket 写到数据目录；`zero-server ticket` 也是读这个文件。
> 不要用 `ticket --bind` 对着正在运行的服务生成——那会顶掉它的 relay 连接。

也可以填裸 endpoint id（`zero-server id`），客户端会自动重试等发现记录。

## 手动安装

参考 `packaging/zero-server.service`，把二进制放到 `/usr/local/bin/zero-server`，
数据目录放 `/var/lib/zero-server`，然后：

```bash
systemctl daemon-reload
systemctl enable --now zero-server
```
