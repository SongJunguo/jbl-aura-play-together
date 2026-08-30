# Rust 整对后端边界

本边界最初用于 Rust 单写者状态机的 Stage C/D 离线实现；截至 2026-08-30，原生
`NativePair` 已完成受控实机 checkpoint。本文只定义事务与证据语义，不把一次设备接受
升级为声学或标准数据面证明。

## 后端是完整设备对，不是半台音响

`PairBackend` 的一次 `start`、`stop` 或 `shutdown` 必须拥有 JBL Authentics 300 与
Aura Studio 5 的完整生命周期操作：

- `LegacyV04WholePair` 调用 v0.4 持久守护进程的本地 Unix socket。旧守护进程内部已经
  同时持有 JBL 与 Aura 控制会话并执行完整顺序，因而绝不能把它伪装成 Aura 半边，再
  与 Rust HTTPS JBL 写入混合；
- `NativePair` 在自身内部组合经过验证的 JBL HTTPS 与 BlueZ Aura 通道，但对上层仍然
  只暴露一个完整设备对事务；Home-flow-only 无 `7957` 设计已被目标方向声学反证。
  exact GATT 实现把独立 Assistant 的 JBL `7957` broadcaster 语义与 Aura AA receiver
  语义组合在同一整对事务内；这是跨两个官方状态机的方向化组合，不伪称同一 UI 序列；
- 一项控制事务从开始到结果确定都固定同一个后端。超时或结果不确定时，不得在同一
  事务中切换到另一后端并重复写入。只能结束本次事务，重新读取安全状态后由操作者
  显式决定下一步。

`PairBackendTransaction` 在调用前后检查后端类型，阻止运行中静默切换。后端回复只
投影为 allowlist 生命周期、粗粒度健康状态、是否存在错误和 Aura 传输枚举；原始错误、
路径、PID、证据文本与未知字段不会离开兼容客户端。

## 写入结果不是普通 `Result`

整对生命周期 action 使用封闭的 `PairActionResult`：

- `Accepted`：兼容守护进程返回结构有效、`ok:true`，且 allowlist 生命周期等于该命令
  的预期后置状态；这仍只是本地生命周期 ACK，不是双成员实时拓扑或双响证明；
- `RejectedBeforeSend`：仅用于连接、socket 信任检查或固定后端检查在首次命令写入尝试
  前失败。只有这一分支能证明本次调用没有尝试发送命令字节；
- `OutcomeUnknown`：从第一次 `write` 调用开始，超时、断连、畸形或超大回复、
  `ok:false`、未知/错误后置生命周期以及 action 后发现后端变化全部归入这里。错误原因
  只用于诊断，绝不能被当作自动重试或切换后端的依据。

若回复中带有可识别的 allowlist 生命周期，即使 ACK 为负或后置状态不符，仍以
`observed_lifecycle` 保存这条经过清洗的局部证据；未知字符串、原始错误和其他字段不会
进入结果。`PairBackendTransaction` 在第一次 `OutcomeUnknown` 后闭锁：同一事务里的后续
`start/stop/shutdown` 直接返回同一条不确定结果，不再调用后端。状态机可以执行只读状态
核查，但必须显式结束该事务，并根据安全状态决定新事务；不得因为超时或错误类别自动
failover 或重发。

Rust 日常服务只构造 `NativePair`。v0.4 的独立 launcher/服务仍保留为人工明确选择的
回退，不是 Rust 状态机的自动备用线路；一次 Rust 写前拒绝、timeout、断连或结果未知
都不得触发 v0.4 写入。

## 跨版本单写者与显式切换

两个版本共享 `$XDG_RUNTIME_DIR/jbl-aura-link` 这一 owner-only 信任根：Rust `serve`
全生命周期同时持有 `operation.lock` 与 `session.lock`，并尊重 v0.4 的
`launch.reservation`；v0.4 的所有公开入口先取 operation 锁，Python 持久 supervisor
全生命周期持有 session 锁。因此直接运行、systemd 启动和 v0.4 启动交接三种路径都只
能有一个设备写者，竞争者在构造/调用设备后端前失败。

安装器不会自动替用户切换：启用 Rust 时若 v0.4 unit 仍 enabled 会拒绝；v0.4
`install-service` 发现 Rust unit enabled 也会在改文件或设备状态前拒绝。从 v0.4 切到
Rust 的明确顺序是：先 `jbl-aura-link shutdown`，再 disable/stop
`jbl-aura-link-session.service`，随后执行 Rust 安装器 `--enable` 并启动 Rust unit。切回
时先执行 `jbl-aura-link-rust stop`，再 disable/stop Rust unit，最后明确安装/启用 v0.4。
共享锁只负责 fail-closed，不会自动 failover、补写或替操作者决定切换。

## v0.4 兼容协议

现有 `lib/jbl_aura_session.py` 的控制 socket 是“一连接、一命令、一行 JSON 回复”：

```text
status\n
start\n
stop\n
shutdown\n
```

Rust `LegacyV04PairBackend` 因此每次只建立一个短 Unix socket 连接，发送一个固定命令，
关闭写半边，再读取一个有大小上限的单行回复。它没有默认 socket 路径、环境变量查找、
runtime 目录探测、守护进程拉起或 systemd 操作；构造时必须显式传入绝对路径，且构造
本身不访问文件系统。兼容协议已用临时 mock socket 完成离线验证；当前 Rust 日常服务
不接入该 socket，而是与独立保留的 v0.4 launcher 并存。

Ubuntu 兼容客户端在实际请求前用 `lstat` 拒绝符号链接、非 socket、非当前 euid 所有或
权限不是 `0600` 的端点；连接使用 `O_NONBLOCK | O_CLOEXEC`、绝对截止时间、
`poll/SO_ERROR`，并在连接后用 `SO_PEERCRED` 再核对 peer euid。读写也各有超时，错误
只返回固定类型，不回显 socket 路径、peer 身份或守护进程原始文本。临时测试覆盖无
listener 的陈旧 socket、已满 backlog、宽权限、符号链接、超时和畸形/超限回复。

兼容回复中的控制 ACK 仍然不等于设备报告的双成员配置，更不等于人工双响或
BASS/BIG/BIS/ISO 数据面证明。上层状态机须读取 JBL 成员配置来排除错误设备，但实机
已证明该配置在 STOP 后仍保留，所以不能把它当作实时 linked/unlinked 后置条件。

## 写前日志与实机 checkpoint

单写者 Controller 在调用 `PairBackend` 的可变 action 前，先把不含设备身份的 pending
记录持久化。只有 `Accepted` 且新读成员配置仍精确指向预期设备对时才清除。若进程在
action 边界崩溃，或后端返回 `OutcomeUnknown`，该记录跨重启保留，普通
`start/stop/shutdown` 继续闭锁；不能靠切换 v0.4 绕过。

2026-08-30 的受控实机结果与这个边界一致：

- 一次真实 `stop` 在 FDDF 空窗中无法证明 Aura 身份，因而
  `RejectedBeforeSend`，未写设备并安全清回 `clean`；
- 旧 timeout 构造曾在 pending 已落盘后触发真实 panic，重启仍看到 pending；该 panic
  已修，但 uncertainty 没有被重启偷偷清除；
- 直接连接已发现的 LE `Device1` 失败后，已配对且 trusted 的稳定 public 对象触发
  BlueZ 连接；vendor GATT 实际位于唯一 connected random 对象，只有其 FDDF 精确匹配
  PID 与内嵌稳定身份才采用；显式恢复最终 `Accepted` 并回到 `ready`；
- 原生 Rust `start/stop` 获得设备接受；两轮无按键冷 `start` 的 managed 状态第一轮曾
  报 `br_edr`、第二轮最终 `le`；持久 session 的两次正常 `stop` 约为 0.44 与 0.57 秒。

约 `03:45` 的完整歌曲尝试与 Home Centre 自动 STOP 重叠，JBL 单响结果受并发 writer
污染，不能作为协议负例。后来的 EOF-fixed clean Rust Home-flow-only 事务不发送
`7957`：Aura AA ON 与 JBL Wi-Fi ENTER accepted/local linked，等待 `15` 秒再仅向 JBL
网络播放仍只有 JBL 出声。这是目标方向的有效负例，并排除原 `2` 秒过短是唯一原因；
固定 `10.5`/`15` 秒不是修复。simultaneous official run 则是 Android A2DP→Aura，
Aura PRIMARY/JBL RECEIVER。目标 JBL-source 因而重新纳入独立 Assistant `7957`
broadcaster 语义，但必须保持两个状态机及证据来源分离。exact GATT `0x002a` 首轮
START accepted，MA 仅向 JBL 网络以请求的 `5%` 播放时用户确认双响；普通 STOP
aura_ack_timeout/outcome-unknown，显式 recover-stop 在 `13` 秒内 accepted/ready。
fresh-bearer 修复后第二轮再次双响，音乐 idle 后普通 STOP 约 `43` 秒 accepted/ready、
无需 recovery。按约定停止声音测试；整对后端仍缺 `7951`、A2DP
`wake_then_stable` 子路径与发布验收，因此 P0 尚未完成。production no-button cold 已
通过一轮 `fresh_le` 实机验收；该 A2DP 子路径尚未命中/证明。
