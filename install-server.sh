#!/usr/bin/env bash
#
# zero-server 一键部署：
#   - 创建系统用户
#   - 安装二进制到 /usr/local/bin
#   - 生成服务端密钥和 authorized_keys
#   - 写 systemd unit 并启动
#
# 用法：
#   sudo ./install-server.sh [选项]
#
# 选项：
#   --bin <path>        指定 zero-server 二进制（默认找 ./zero-server 或 target/release）
#   --download          没有本地二进制时从 GitHub Release 下载（默认就会尝试下载）
#   --no-download       只用本地二进制，不联网
#   --arch <arch>       覆盖架构检测（x86_64 / aarch64）
#   --user <name>       运行用户（默认 zero）
#   --data-dir <path>   数据目录（默认 ~/.local/share/zero-server）
#   --allow-any         调试用：允许任何客户端（默认走白名单）
#   --download          没有本地二进制时，从 GitHub Release 下载（默认 latest）
#   --version <tag>     配合 --download 指定版本（例如 v0.1.0）
#   --uninstall         停止并移除服务
#   --purge             卸载时连数据目录一起删
#   --dry-run           只打印将要执行的命令
#   -h, --help          帮助

set -euo pipefail

PREFIX="/usr/local/bin"
SERVICE_NAME="zero-server"
SERVICE_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
DATA_DIR=""
RUN_USER=""
BIN_SRC=""
ALLOW_ANY=0
DOWNLOAD=1
VERSION="latest"
ARCH_OVERRIDE=""
UNINSTALL=0
PURGE=0
DRY_RUN=0

usage() {
  cat <<'USAGE'
zero-server 一键部署（systemd）

用法：
  sudo ./install-server.sh [选项]

选项：
  --bin <path>        指定本地 zero-server 二进制
  --user <name>       运行用户（默认执行 sudo 的当前用户）
  --data-dir <path>   数据目录（默认 ~/.local/share/zero-server）
  --arch <arch>       覆盖架构检测（x86_64 / aarch64）
  --version <tag>     指定下载版本（默认 latest）
  --allow-any         调试用：允许任何客户端（默认走白名单）
  --download          从 GitHub Release 下载（默认行为，这个参数可省略）
  --no-download       只用本地二进制，不联网
  --uninstall         停止并移除服务
  --purge             卸载时连数据目录一起删
  --dry-run           只打印将要执行的命令
  -h, --help          帮助
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin) BIN_SRC="${2:?--bin 需要参数}"; shift 2 ;;
    --user) RUN_USER="${2:?--user 需要参数}"; shift 2 ;;
    --data-dir) DATA_DIR="${2:?--data-dir 需要参数}"; shift 2 ;;
    --allow-any) ALLOW_ANY=1; shift ;;
    --download) DOWNLOAD=1; shift ;;
    --no-download) DOWNLOAD=0; shift ;;
    --arch) ARCH_OVERRIDE="${2:?--arch 需要参数}"; shift 2 ;;
    --version) VERSION="${2:?--version 需要参数}"; shift 2 ;;
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
  if [[ "${EUID}" -ne 0 ]]; then
    echo "需要 root 权限，请用 sudo 运行。" >&2
    exit 1
  fi
}

resolve_user() {
  if [[ -z "$RUN_USER" ]]; then
    RUN_USER="${SUDO_USER:-$(id -un)}"
  fi
  if ! id "$RUN_USER" >/dev/null 2>&1; then
    echo "用户不存在：$RUN_USER" >&2
    exit 1
  fi
  RUN_GROUP="$(id -gn "$RUN_USER")"
  RUN_HOME="$(getent passwd "$RUN_USER" | cut -d: -f6)"
  if [[ -z "$RUN_HOME" || ! -d "$RUN_HOME" ]]; then
    echo "用户 $RUN_USER 没有可用的 home 目录（$RUN_HOME）" >&2
    exit 1
  fi
  if [[ -z "$DATA_DIR" ]]; then
    DATA_DIR="$RUN_HOME/.local/share/zero-server"
  fi
  if [[ "$RUN_USER" == "root" ]]; then
    echo "警告：将以 root 身份运行远端 shell。建议用普通用户执行 sudo 安装。" >&2
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

need_systemd() {
  if ! command -v systemctl >/dev/null 2>&1; then
    echo "没有 systemctl，这个脚本只支持 systemd 系统。" >&2
    exit 1
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
    echo "==> 删除数据目录 $DATA_DIR"
    run rm -rf "$DATA_DIR"
  else
    echo "保留数据目录：$DATA_DIR（加 --purge 可一起删）"
  fi
  echo "已卸载。"
}

locate_binary() {
  if [[ -n "$BIN_SRC" ]]; then
    return
  fi
  for candidate in ./zero-server ./target/release/zero-server; do
    if [[ -f "$candidate" ]]; then
      BIN_SRC="$candidate"
      return
    fi
  done
}

detect_arch() {
  if [[ -n "$ARCH_OVERRIDE" ]]; then
    echo "$ARCH_OVERRIDE"
    return
  fi
  local machine
  machine="$(uname -m)"
  case "$machine" in
    x86_64|amd64) echo "x86_64" ;;
    aarch64|arm64) echo "aarch64" ;;
    *) echo "unsupported:$machine" ;;
  esac
}

download_binary() {
  local arch
  arch="$(detect_arch)"
  if [[ "$arch" == unsupported:* ]]; then
    echo "不支持的架构：${arch#unsupported:}。请从源码构建：cargo build --release -p zero-server" >&2
    exit 1
  fi
  if [[ "$(uname -s)" != "Linux" ]]; then
    echo "一键部署只支持 Linux（当前 $(uname -s)），其它平台请从源码构建。" >&2
    exit 1
  fi
  command -v curl >/dev/null 2>&1 || {
    echo "需要 curl 才能下载" >&2
    exit 1
  }
  local tag="$VERSION"
  if [[ "$tag" == "latest" ]]; then
    tag="$(curl -fsSL https://api.github.com/repos/lipeilin2006/zero/releases/latest | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
  fi
  [[ -n "$tag" ]] || { echo "拿不到最新版本号" >&2; exit 1; }
  local url="https://github.com/lipeilin2006/zero/releases/download/${tag}/zero-server-${tag}-${arch}-linux.tar.gz"
  echo "==> 下载 $url"
  local tmp
  tmp="$(mktemp -d)"
  curl -fL "$url" -o "$tmp/zero-server.tar.gz"
  tar -xzf "$tmp/zero-server.tar.gz" -C "$tmp"
  BIN_SRC="$tmp/zero-server"
  DOWNLOAD_DIR="$tmp"
}

install_service() {
  need_root
  need_systemd
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
    echo "找不到 zero-server 二进制。" >&2
    echo "先构建：cargo build --release -p zero-server" >&2
    echo "或用 --bin 指定路径；默认会尝试从 GitHub Release 下载，离线时加 --no-download。" >&2
    exit 1
  fi
  if [[ ! -f "$BIN_SRC" && "$DRY_RUN" -eq 0 ]]; then
    echo "二进制不存在：$BIN_SRC" >&2
    exit 1
  fi

  resolve_user
  echo "==> 运行用户 $RUN_USER（$RUN_HOME）/ 数据目录 $DATA_DIR"
  # 数据目录在 home 下时，把缺失的中间层也建给该用户，
  # 否则客户端写 ~/.local/share/zero 会被 root 建的目录挡住。
  if [[ "$DATA_DIR" == "$RUN_HOME/"* ]]; then
    parent="$(dirname "$DATA_DIR")"
    while [[ "$parent" != "$RUN_HOME" && "$parent" != "/" ]]; do
      if [[ ! -d "$parent" ]]; then
        run install -d -m 0755 -o "$RUN_USER" -g "$RUN_GROUP" "$parent"
      elif [[ "$(stat -c %U "$parent" 2>/dev/null || true)" == "root" ]]; then
        # 之前被 root 建过（比如首次安装时），交还给用户
        run chown "$RUN_USER:$RUN_GROUP" "$parent"
      fi
      parent="$(dirname "$parent")"
    done
  fi
  run install -d -m 0750 -o "$RUN_USER" -g "$RUN_GROUP" "$DATA_DIR"

  echo "==> 安装二进制"
  run install -m 0755 "$BIN_SRC" "$PREFIX/zero-server"

  echo "==> 初始化密钥和 allowlist"
  run touch "$DATA_DIR/authorized_keys"
  run chown "$RUN_USER:$RUN_GROUP" "$DATA_DIR/authorized_keys"
  run chmod 0600 "$DATA_DIR/authorized_keys"
  run chown "$RUN_USER:$RUN_GROUP" "$DATA_DIR"
  run chmod 0750 "$DATA_DIR"

  if [[ ! -f "$DATA_DIR/secret.key" ]]; then
    if [[ "$DRY_RUN" -eq 1 ]]; then
      echo "+ as $RUN_USER: $PREFIX/zero-server id --key $DATA_DIR/secret.key"
    else
      as_user "$PREFIX/zero-server" id --key "$DATA_DIR/secret.key" >/dev/null
    fi
  fi
  run chown "$RUN_USER:$RUN_GROUP" "$DATA_DIR/secret.key"
  run chmod 0600 "$DATA_DIR/secret.key"

  if [[ "$ALLOW_ANY" -eq 1 ]]; then
    EXEC_ARGS="run --key $DATA_DIR/secret.key --allow-all"
  else
    EXEC_ARGS="run --key $DATA_DIR/secret.key --allowlist $DATA_DIR/authorized_keys"
  fi

  echo "==> 写入 systemd unit：$SERVICE_PATH"
  UNIT_CONTENT="[Unit]
Description=Zero remote shell server (iroh)
Documentation=https://github.com/lipeilin2006/zero
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$RUN_USER
Group=$RUN_GROUP
WorkingDirectory=$RUN_HOME
Environment=HOME=$RUN_HOME
ExecStart=$PREFIX/zero-server $EXEC_ARGS
Restart=on-failure
RestartSec=2
NoNewPrivileges=true
ProtectSystem=full
PrivateTmp=true
ReadWritePaths=$DATA_DIR
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
LockPersonality=true
MemoryDenyWriteExecute=true
SystemCallArchitectures=native
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
"

  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "----- unit 内容 -----"
    echo "$UNIT_CONTENT"
    echo "---------------------"
  else
    printf '%s' "$UNIT_CONTENT" > "$SERVICE_PATH"
  fi

  run systemctl daemon-reload
  run systemctl enable --now "$SERVICE_NAME"

  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "==> 完成（dry-run，没有真正执行）"
    return
  fi

  sleep 1
  echo "==> 服务状态"
  systemctl is-active "$SERVICE_NAME" || true
  journalctl -u "$SERVICE_NAME" --no-pager -n 5 || true

  echo
  echo "==> 部署完成"
  echo "服务器 endpoint id："
  as_user "$PREFIX/zero-server" id --key "$DATA_DIR/secret.key" | head -1
  echo
  echo "客户端填这个 ticket（带地址，推荐）："
  for _ in $(seq 1 120); do
    [[ -s "$DATA_DIR/ticket" ]] && break
    sleep 0.5
  done
  if [[ -s "$DATA_DIR/ticket" ]]; then
    cat "$DATA_DIR/ticket"
  else
    echo "（ticket 还没生成，稍后再看：cat $DATA_DIR/ticket）"
  fi
  echo
  echo "信任客户端（客户端 id 用 zero --show-id 取）："
  echo "  sudo -u $RUN_USER $PREFIX/zero-server trust <client-id> \\"
  echo "    --key $DATA_DIR/secret.key --allowlist $DATA_DIR/authorized_keys"
  echo
  echo "看日志：journalctl -u $SERVICE_NAME -f"
  echo "卸载：./install-server.sh --uninstall [--purge]"

  if [[ -n "${DOWNLOAD_DIR:-}" ]]; then
    rm -rf "$DOWNLOAD_DIR"
  fi
}

if [[ "$UNINSTALL" -eq 1 ]]; then
  uninstall
else
  install_service
fi
