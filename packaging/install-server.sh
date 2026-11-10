#!/usr/bin/env bash
#
# zero-server 一键部署：
#   - 创建系统用户
#   - 安装二进制到 /usr/local/bin
#   - 生成服务端密钥和 authorized_keys
#   - 写 systemd unit 并启动
#
# 用法：
#   sudo packaging/install-server.sh [选项]
#
# 选项：
#   --bin <path>        指定 zero-server 二进制（默认找 ./zero-server 或 target/release）
#   --user <name>       运行用户（默认 zero）
#   --data-dir <path>   数据目录（默认 /var/lib/zero-server）
#   --allow-any         调试用：允许任何客户端（默认走白名单）
#   --uninstall         停止并移除服务
#   --purge             卸载时连数据目录一起删
#   --dry-run           只打印将要执行的命令
#   -h, --help          帮助

set -euo pipefail

PREFIX="/usr/local/bin"
SERVICE_NAME="zero-server"
SERVICE_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
DATA_DIR="/var/lib/zero-server"
RUN_USER="zero"
BIN_SRC=""
ALLOW_ANY=0
UNINSTALL=0
PURGE=0
DRY_RUN=0

usage() {
  sed -n '3,24p' "$0" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin) BIN_SRC="${2:?--bin 需要参数}"; shift 2 ;;
    --user) RUN_USER="${2:?--user 需要参数}"; shift 2 ;;
    --data-dir) DATA_DIR="${2:?--data-dir 需要参数}"; shift 2 ;;
    --allow-any) ALLOW_ANY=1; shift ;;
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

need_systemd() {
  if ! command -v systemctl >/dev/null 2>&1; then
    echo "没有 systemctl，这个脚本只支持 systemd 系统。" >&2
    exit 1
  fi
}

uninstall() {
  need_root
  need_systemd
  echo "==> 停止并移除 $SERVICE_NAME"
  run systemctl disable --now "$SERVICE_NAME" || true
  run rm -f "$SERVICE_PATH"
  run systemctl daemon-reload
  run rm -f "$PREFIX/zero-server"
  if [[ "$PURGE" -eq 1 ]]; then
    echo "==> 删除数据目录 $DATA_DIR"
    run rm -rf "$DATA_DIR"
    if id "$RUN_USER" >/dev/null 2>&1; then
      run userdel "$RUN_USER" || true
    fi
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

install_service() {
  need_root
  need_systemd
  locate_binary

  if [[ -z "$BIN_SRC" ]]; then
    echo "找不到 zero-server 二进制。" >&2
    echo "先构建：cargo build --release -p zero-server" >&2
    echo "或用 --bin 指定路径。" >&2
    exit 1
  fi
  if [[ ! -f "$BIN_SRC" && "$DRY_RUN" -eq 0 ]]; then
    echo "二进制不存在：$BIN_SRC" >&2
    exit 1
  fi

  echo "==> 用户 $RUN_USER / 数据目录 $DATA_DIR"
  if ! id "$RUN_USER" >/dev/null 2>&1; then
    NOLOGIN="/usr/sbin/nologin"
    [[ -x "$NOLOGIN" ]] || NOLOGIN="/sbin/nologin"
    run useradd --system --home-dir "$DATA_DIR" --shell "$NOLOGIN" "$RUN_USER"
  else
    echo "用户已存在，跳过创建。"
  fi
  run install -d -m 0750 -o "$RUN_USER" -g "$RUN_USER" "$DATA_DIR"

  echo "==> 安装二进制"
  run install -m 0755 "$BIN_SRC" "$PREFIX/zero-server"

  echo "==> 初始化密钥和 allowlist"
  run touch "$DATA_DIR/authorized_keys"
  run chown "$RUN_USER:$RUN_USER" "$DATA_DIR/authorized_keys"
  run chmod 0600 "$DATA_DIR/authorized_keys"
  run chown "$RUN_USER:$RUN_USER" "$DATA_DIR"
  run chmod 0750 "$DATA_DIR"

  if [[ ! -f "$DATA_DIR/secret.key" ]]; then
    if [[ "$DRY_RUN" -eq 1 ]]; then
      echo "+ sudo -u $RUN_USER $PREFIX/zero-server id --key $DATA_DIR/secret.key"
    else
      sudo -u "$RUN_USER" "$PREFIX/zero-server" id --key "$DATA_DIR/secret.key" >/dev/null
    fi
  fi
  run chown "$RUN_USER:$RUN_USER" "$DATA_DIR/secret.key"
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
Group=$RUN_USER
ExecStart=$PREFIX/zero-server $EXEC_ARGS
Restart=on-failure
RestartSec=2
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$DATA_DIR
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
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
  sudo -u "$RUN_USER" "$PREFIX/zero-server" id --key "$DATA_DIR/secret.key" | head -1
  echo
  echo "把带地址的 ticket 填到客户端（推荐）："
  echo "  sudo -u $RUN_USER $PREFIX/zero-server ticket --key $DATA_DIR/secret.key"
  echo
  echo "信任客户端（客户端 id 用 zero --show-id 取）："
  echo "  sudo -u $RUN_USER $PREFIX/zero-server trust <client-id> \\"
  echo "    --key $DATA_DIR/secret.key --allowlist $DATA_DIR/authorized_keys"
  echo
  echo "看日志：journalctl -u $SERVICE_NAME -f"
  echo "卸载：$0 --uninstall [--purge]"
}

if [[ "$UNINSTALL" -eq 1 ]]; then
  uninstall
else
  install_service
fi
