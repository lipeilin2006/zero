# zero 代码审查（客户端多标签终端改造时做的）

## 已修

| 位置 | 问题 | 处理 |
|---|---|---|
| `zero-server` | 会话数量没有上限，连接一多就把 PTY 和内存吃满 | 信号量限制 32 个并发会话，超出直接拒绝 |
| `zero-server` / `zero` | 密钥文件默认 0644，同机其他用户可读走私钥 | 创建时 chmod 0600（Unix） |
| `zero-client` | 服务器列表写文件不是原子的，写到一半崩会丢列表 | 先写 `.tmp` 再 rename |
| `zero-client` | Tauri 的 CSP 是 `null` | 配了 `default-src 'self'` 的 CSP |
| `zero` | 服务器正常关连接时打印 `Error: connection lost` 并退出码非 0 | 当成正常断开处理 |
| `zero` | 原来不在终端里跑就报错 | 未改；需要 TTY，客户端内嵌后不再走 CLI |

## 还没做

| 位置 | 问题 | 建议 |
|---|---|---|
| `zero-server` | 没有空闲超时：连接挂着不操作就一直占一个会话 | 加 30 分钟无 IO 自动断开 |
| `zero-server` | 没有连接频率限制 | 简单令牌桶，或按 peer 计失败次数 |
| `zero-server` | `authorized_keys` 只在启动时读，改完要重启 | 文件 mtime 变化时热加载 |
| `zero-proto` | `read_msg` 先按对端长度分配最多 16MB | 可以把上限降到 1MB，或改成流式 |
| `zero-client` | 终端输出直接逐条推到前端，命令狂刷屏时事件太多 | 前端做 16ms 批量合并 |
| `zero-client` | `launch_zero` 会优先用 `ZERO_BIN` 环境变量 | 发行包里建议去掉这个后门 |
| 前端 | naive-ui 全量引入，单包 1.58MB | 改成按需引入，或代码分割 Terminal 组件 |
| 前端 | ~~行模式终端，跑不了 vim/top~~ | 已换成 xterm.js，PTY 字节直接转发 |

## 安全模型

- 认证 = iroh 端点身份 + 服务端 `authorized_keys`（按客户端公钥）
- 没有密码、没有转发、没有端口转发，只有 shell
- `--allow-all` 等于谁都能连，只适合本机调试
- 客户端密钥就是身份，泄露等于把 shell 给别人，务必 0600
