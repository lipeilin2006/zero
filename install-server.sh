#!/usr/bin/env bash
#
# zero-server 一键全局安装（systemd）：
#   - 安装到 /usr/local/bin
#   - 配置和密钥统一放 /etc/zero/server
#   - 自动创建专用 zero 用户、systemd unit 并启动
#
# 用法：
#   sudo ./install-server.sh [选项]
#
# 选项：
#   --bin <path>        指定本地 zero-server 二进制
#   --config-dir <path> 配置目录（默认 /etc/zero/server）
#   --user <name>       运行用户（默认 zero，不存在会自动创建）
#   --arch <arch>       覆盖架构检测（x86_64 / aarch64）
#   --version <tag>     指定下载版本（默认 latest）
#   --allow-any         调试用：允许任何客户端（默认走白名单）
#   --hardened          更严的 systemd 沙箱（会禁止 su/sudo）
#   --download          从 GitHub Release 下载（默认行为，可省略）
#   --no-download       只用本地二进制，不联网
#   --uninstall         停止并移除服务和二进制
#   --purge             卸载时连配置目录和用户一起删
#   --dry-run           只打印将要执行的命令
#   -h, --help          帮助

set -euo pipefail

PREFIX="/usr/local/bin"
SERVICE_NAME="zero-server"
SERVICE_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
CONFIG_DIR="/etc/zero/server"
CONFIG_FILE="$CONFIG_DIR/config.json"
RUN_USER="zero"
BIN_SRC=""
ALLOW_ANY=0
DOWNLOAD=1
VERSION="latest"
ARCH_OVERRIDE=""
UNINSTALL=0
PURGE=0
DRY_RUN=0
HARDENED=0

usage() {
  cat <<'USAGE'
zero-server 一键全局安装（systemd）

用法：
  sudo ./install-server.sh [选项]

选项：
  --bin <path>        指定本地 zero-server 二进制
  --config-dir <path> 配置目录（默认 /etc/zero/server）
  --user <name>       运行用户（默认 zero，不存在会自动创建）
  --arch <arch>       覆盖架构检测（x86_64 / aarch64，默认自动）
  --version <tag>     指定下载版本（默认 latest）
  --allow-any         调试用：允许任何客户端（默认走白名单）
  --hardened          更严的沙箱（会禁止 su/sudo）
  --download          从 GitHub Release 下载（默认行为，可省略）
  --no-download       只用本地二进制，不联网
  --uninstall         停止并移除服务和二进制
  --purge             卸载时连配置目录和用户一起删
  --dry-run           只打印将要执行的命令
  -h, --help          帮助
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin) BIN_SRC="${2:?--bin 需要参数}"; shift 2 ;;
    --config-dir) CONFIG_DIR="${2:?--config-dir 需要参数}"; CONFIG_FILE="$CONFIG_DIR/config.json"; shift 2 ;;
    --user) RUN_USER="${2:?--user 需要参数}"; shift 2 ;;
    --arch) ARCH_OVERRIDE="${2:?--arch 需要参数}"; shift 2 ;;
    --version) VERSION="${2:?--version 需要参数}"; shift 2 ;;
    --allow-any) ALLOW_ANY=1; shift ;;
    --hardened) HARDENED=1; shift ;;
    --download) DOWNLOAD=1; shift ;;
    --no-download) DOWNLOAD=0; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    --purge) PURGE=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "未知参数：$1"; usage; exit 1 ;;
  esac
done

run() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "+ $*"
  else
    "$@"
  fi
}

need_root() {
  [[ "${EUID}" -eq 0 ]] || { echo "需要 root 权限，请用 sudo 运行。" >&2; exit 1; }
}

need_systemd() {
  command -v systemctl >/dev/null 2>&1 || {
    echo "没有 systemctl，这个脚本只支持 systemd 系统。" >&2
    exit 1
  }
}

resolve_user() {
  if ! id "$RUN_USER" >/dev/null 2>&1; then
    local nologin="/usr/sbin/nologin"
    [[ -x "$nologin" ]] || nologin="/sbin/nologin"
    run useradd --system --home-dir "$CONFIG_DIR" --shell "$nologin" "$RUN_USER"
  fi
  if id "$RUN_USER" >/dev/null 2>&1; then
    RUN_GROUP="$(id -gn "$RUN_USER")"
  else
    # dry-run 时用户还没创建
    RUN_GROUP="$RUN_USER"
  fi
}

as_user() {
  if [[ "$RUN_USER" == "root" ]]; then
    "$@"
  elif command -v runuser >/dev/null 2>&1; then
    runuser -u "$RUN_USER" -- "$@"
  else
    sudo -u "$RUN_USER" -- "$@"
  fi
}

detect_arch() {
  if [[ -n "$ARCH_OVERRIDE" ]]; then
    echo "$ARCH_OVERRIDE"
    return
  fi
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    aarch64|arm64) echo "aarch64" ;;
    *) echo "unsupported:$(uname -m)" ;;
  esac
}

locate_binary() {
  [[ -n "$BIN_SRC" ]] && return
  for candidate in ./zero-server ./target/release/zero-server; do
    if [[ -f "$candidate" ]]; then
      BIN_SRC="$candidate"
      return
    fi
  done
}

download_binary() {
  local arch tag url tmp
  arch="$(detect_arch)"
  if [[ "$arch" == unsupported:* ]]; then
    echo "不支持的架构：${arch#unsupported:}。请从源码构建。" >&2
    exit 1
  fi
  if [[ "$(uname -s)" != "Linux" ]]; then
    echo "一键部署只支持 Linux（当前 $(uname -s)）。" >&2
    exit 1
  fi
  command -v curl >/dev/null 2>&1 || { echo "需要 curl 才能下载" >&2; exit 1; }

  tag="$VERSION"
  if [[ "$tag" == "latest" ]]; then
    tag="$(curl -fsSL https://api.github.com/repos/lipeilin2006/zero/releases/latest \
      | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
  fi
  [[ -n "$tag" ]] || { echo "拿不到最新版本号" >&2; exit 1; }

  url="https://github.com/lipeilin2006/zero/releases/download/${tag}/zero-server-${tag}-${arch}-linux.tar.gz"
  echo "==> 下载 $url"
  tmp="$(mktemp -d)"
  curl -fL "$url" -o "$tmp/zero-server.tar.gz"
  tar -xzf "$tmp/zero-server.tar.gz" -C "$tmp"
  BIN_SRC="$tmp/zero-server"
  DOWNLOAD_DIR="$tmp"
}

write_config() {
  if [[ -f "$CONFIG_FILE" ]]; then
    echo "配置已存在，保留：$CONFIG_FILE"
    return
  fi
  local allow=false
  [[ "$ALLOW_ANY" -eq 1 ]] && allow=true
  local content
  content="{
  \"key\": \"$CONFIG_DIR/secret.key\",
  \"allowlist\": \"$CONFIG_DIR/authorized_keys\",
  \"allow_all\": $allow
}"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "+ write $CONFIG_FILE:"
    echo "$content"
  else
    printf '%s\n' "$content" > "$CONFIG_FILE"
    chown "root:$RUN_GROUP" "$CONFIG_FILE"
    chmod 0640 "$CONFIG_FILE"
  fi
}

install_service() {
  need_root
  need_systemd
  resolve_user
  locate_binary

  if [[ -z "$BIN_SRC" && "$DOWNLOAD" -eq 1 ]]; then
    if [[ "$DRY_RUN" -eq 1 ]]; then
      echo "+ download zero-server ${VERSION} ($(detect_arch)-linux)"
      BIN_SRC="(downloaded zero-server)"
    else
      download_binary
    fi
  fi
  if [[ -z "$BIN_SRC" ]]; then
    echo "找不到 zero-server 二进制：用 --bin 指定，或先构建。" >&2
    exit 1
  fi

  echo "==> 配置目录 $CONFIG_DIR（用户 $RUN_USER）"
  run install -d -m 0750 -o "$RUN_USER" -g "$RUN_GROUP" "$CONFIG_DIR"

  echo "==> 安装二进制到 $PREFIX/zero-server"
  run install -m 0755 "$BIN_SRC" "$PREFIX/zero-server"

  echo "==> 写配置"
  write_config

  echo "==> 初始化 allowlist / secret key"
  run touch "$CONFIG_DIR/authorized_keys"
  run chown "$RUN_USER:$RUN_GROUP" "$CONFIG_DIR/authorized_keys"
  run chmod 0600 "$CONFIG_DIR/authorized_keys"

  if [[ ! -f "$CONFIG_DIR/secret.key" ]]; then
    if [[ "$DRY_RUN" -eq 1 ]]; then
      echo "+ as $RUN_USER: $PREFIX/zero-server -c $CONFIG_FILE id"
    else
      as_user "$PREFIX/zero-server" -c "$CONFIG_FILE" id >/dev/null
    fi
  fi
  run chown "$RUN_USER:$RUN_GROUP" "$CONFIG_DIR/secret.key"
  run chmod 0600 "$CONFIG_DIR/secret.key"

  echo "==> 写 systemd unit"
  local sandbox
  if [[ "$HARDENED" -eq 1 ]]; then
    sandbox="NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
MemoryDenyWriteExecute=true
PrivateTmp=true
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
LockPersonality=true
SystemCallArchitectures=native
ReadWritePaths=$CONFIG_DIR"
  else
    # 默认跟 SSH 一样：不做文件系统沙箱。
    # apt / passwd / systemctl 这类命令需要写 /usr、/etc，一旦开
    # ProtectSystem（哪怕是 full）就会报 Read-only file system；
    # su 切换用户也绕不开，因为限制属于整个 service cgroup。
    sandbox=""
  fi
  local unit
  unit="[Unit]
Description=Zero remote shell server (iroh)
Documentation=https://github.com/lipeilin2006/zero
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$RUN_USER
Group=$RUN_GROUP
WorkingDirectory=$CONFIG_DIR
Environment=HOME=$CONFIG_DIR
ExecStart=$PREFIX/zero-server run --config $CONFIG_FILE
Restart=on-failure
RestartSec=2
$sandbox
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "----- unit -----"
    echo "$unit"
    echo "----------------"
  else
    printf '%s' "$unit" > "$SERVICE_PATH"
  fi

  run systemctl daemon-reload
  run systemctl enable --now "$SERVICE_NAME"

  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "==> 完成（dry-run）"
    return
  fi

  sleep 1
  echo "==> 服务状态"
  systemctl is-active "$SERVICE_NAME" || true
  journalctl -u "$SERVICE_NAME" --no-pager -n 4 || true

  echo
  echo "==> 部署完成"
  echo "客户端填这个 endpoint id："
  as_user "$PREFIX/zero-server" -c "$CONFIG_FILE" id
  echo
  echo "常用命令（全局安装，不用带 -c）："
  echo "  zero-server id                    # 服务端 id"
  echo "  zero-server trust <client-id>     # 信任客户端"
  echo "  zero-server trusted               # 查看白名单"
  echo "  zero-server untrust <client-id>   # 取消信任"
  echo "  journalctl -u $SERVICE_NAME -f    # 看日志"
  echo
  echo "卸载：./install-server.sh --uninstall [--purge]"

  if [[ -n "${DOWNLOAD_DIR:-}" ]]; then
    rm -rf "$DOWNLOAD_DIR"
  fi
}

uninstall() {
  need_root
  need_systemd
  resolve_user

  echo "==> 停止并移除 $SERVICE_NAME"
  run systemctl disable --now "$SERVICE_NAME" || true
  run rm -f "$SERVICE_PATH"
  run systemctl daemon-reload
  run rm -f "$PREFIX/zero-server"

  if [[ "$PURGE" -eq 1 ]]; then
    echo "==> 删除配置目录 $CONFIG_DIR"
    run rm -rf "$CONFIG_DIR"
    if id "$RUN_USER" >/dev/null 2>&1; then
      run userdel "$RUN_USER" || true
    fi
  else
    echo "保留配置目录：$CONFIG_DIR（加 --purge 一起删）"
  fi
  echo "已卸载。"
}

if [[ "$UNINSTALL" -eq 1 ]]; then
  uninstall
else
  install_service
fi
