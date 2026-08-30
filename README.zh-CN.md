# Linux 下的 JBL 与琉璃 5 Play Together

[English](README.md)

这是一个非官方、实验性的 Linux 互操作工具，只负责自动关联或解除关联：

- JBL Authentics 300
- Harman Kardon Aura Studio 5（琉璃 5）

它不包含音乐服务器、QQ 音乐、微信登录、云端账号、手机 App 或音频文件，也不会把
Linux 音频复制到两个独立输出。

## 已验证结果与证据边界

2026-08-28，真实设备完成首次声学验收：Ubuntu 不再向琉璃 5 的 A2DP sink 输出，
JBL 的 ATT 传输接受了承载 OneOS `7957 action=1` 的写入，琉璃 5 接受
`aa1304003c0101`，听者确认同一个 JBL 音源在两台音响上都可闻。没有捕获到 JBL
`7951` 应用层成功通知。

后来又在已安装窄防火墙规则、先 SUBSCRIBE 再写 GATT `7957` 的严格试验中等待 15 秒，
当前固件仍没有送达 `7951` 回调。因此 Rust 控制器只允许闭集配置
`JBL_BROADCAST_CONFIRMATION=ack|gena`，这对已实测设备默认使用 `ack`。ACK 模式会明确
返回 `accepted_unconfirmed` 和 `broadcast_acknowledgement_only`，CLI exit `0`；只有
收到匹配的 GENA
动作 33/34 才返回 `accepted` 和 `broadcast_business_notification`。managed `linked`
只表示最近一次被控制器接受的动作，不等同于 `7951` 或声学证明。

窄 callback 防火墙规则已获授权并由用户安装，但 production strict START 仍以
`jbl_broadcast_result_timed_out` 结束，随后用 legacy GATT 归一化；规则不是协议成功
证据。最新 HCI 显示深待机先由 Android 系统 BR/EDR A2DP 自动重连，经 stored link-key
鉴权/加密与 AVDTP Open，约 `2.5` 秒后才出现 FDDF，App LE 读取更晚。wake module 已
完成 production 接入并进入最新 neutral artifact。默认冷链路为：stable raw 一次；仅
在 eligible failure 后执行一次 A2DP ConnectProfile（`20` 秒）；`30` 秒内要求 fresh
FDDF 精确身份/PID；DisconnectProfile 并在 `5` 秒内确认释放；再重试 stable raw 一次；
最后保留原 LE fallback。全程共享 `150` 秒 outer deadline；释放未确认即在角色写前
失败。最新无声无按键冷启动在无活动音频流、journal clean、没有已解析的 BlueZ 现成
device session 条件下，START `122.15` 秒返回 `accepted_unconfirmed`/`linked`；status
为双成员 verified/healthy、Aura route=`fresh_le`；STOP `15.89` 秒返回
`accepted_unconfirmed`/`ready`，最终 journal clean、`NRestarts=0`。手机 App 未参与该
事务，但没有 ADB 手机状态证据。整体 no-button cold path 已在 `150` 秒内实机验收；
A2DP `wake_then_stable` 子路径本轮未单独命中/证明。

当前离线门禁为 `258` 个 library tests、`8` 个 CLI tests（主 harness 共 `266`），另有
FIFO private-file helper `1/1`；audit、deny、fallback、
privacy、neutral 均通过；compatibility evidence mode 已完成。

最终 neutral 重建为 `8,284,440` bytes，最高要求 `GLIBC_2.34`，动态依赖仅 `libc`/
`libgcc`；该新 binary 尚待安装后复核 digest。此前已安装 service checkpoint 为
artifact/installed/process digest 已一致。重启并执行一次只读 status 后，service 为
enabled+active、`NRestarts=0`、managed unknown/offline；`15` 秒 restart-idle 样本为
RSS `8,828 KiB`、`1` thread、`15` fds、平均 CPU `0.0667%`（`1` tick）。loopback
Web/status 页面可读，只显示两个
脱敏成员、allowlisted channel、`last_action`/`age_ms`，CSP/CSRF 保持启用。

随后对 JBL One `2.7.9` 与 Harman Kardon One `2.6.11` 的互操作分析确认：

- JBL 的 `7957` 助手流程把受控 JBL 自身地址放进广播对象，并等待 `7951`；
- Harman Kardon App 会构造完全相同的七字节 ON 帧，并把 `aa00021300` 判定为
  Set Device Info 成功回复；
- 官方 JBL Play Together 页面能够发现琉璃 5、把它放入接收槽，并在页面重进后
  保留该控制状态。

这些事实加强了控制面的解释，但没有识别空口 LE Audio 数据面。尤其要注意，官方
PartyTogether 页面使用 ENTER/EXIT 状态机，而 `7957` 来自 App 内另一条 broadcaster
助手路径；本仓库组合的是两条已确认的厂商控制语义，不宣称逐字节复刻了官方页面的
一次完整事务。

2026-08-29 的真实设备回归还发现并解决了关键生命周期问题：旧的一次性 `start`
成功后，立即运行一次性 `stop` 已无法新建两条控制连接。改为在角色变化前同时建立
连接并持续持有后，同一会话连续完成了 3 次 START 和 2 次 STOP；每次琉璃 ON/OFF
都返回 `aa00021300`，JBL ENTER/EXIT 都返回 `error_code=0`。

v0.4 又补齐了自动冷发现：它不再把会轮换的 LE 地址当成身份，而是通过 BlueZ D-Bus
实时扫描 Harman `FDDF` Service Data，并同时校验 PID 与载荷内嵌的稳定地址。手机已经
断开琉璃、没有按音响按钮时，真实设备完成了两轮 `shutdown → LE cold start → stop`。
其中一轮建立关联后，以 15% 音量只向 JBL 播放，听者再次确认 JBL 与琉璃都响。

边界也有完整记录：两次成功之间，一次 30 秒扫描收到 49 个其他/非身份广告事件，但
没有收到琉璃 FDDF，因此在发送任何角色命令前安全失败；稍后 FDDF 又在 10.7 秒内出现，
下一次冷启动成功。结论因此是：**冷启动已证明可以完全无按键完成，但琉璃广播存在
空窗，不能把每一次立即重试都宣称为必然成功。** 日常仍建议 `stop` 后保持 `ready`。

因此本仓库的准确结论是：**厂商 Play Together 控制序列与双响结果已实证；标准
BASS 建链、实际广播源、BIG/BIS 和精确同步误差仍未证明。** 具体原因包括：

- 琉璃 5 的 BASS Receive State 读取没有得到可解释结果；规范要求加密读取，原探测
  未证明满足了该条件；
- BlueZ 缓存中的 DFFD 样本按 App 私有枚举解码为 `RECEIVER(2)`，但新鲜度未知，
  而且它与 OneOS 中 `2=Broadcaster` 不是同一个数字命名空间；
- 没有捕获 LE Audio 周期广播、BASE、BIGInfo 或 ISO 数据。

详见[成功证据与未决问题](docs/EVIDENCE.md)和[协议记录](docs/PROTOCOL.md)。

后续开发遵守明确的 clean-room、隐私和语言规则，见
[仓库工作规则](AGENTS.md)、[上游功能吸收计划](docs/UPSTREAM_INTAKE.md)与
[语言决策 ADR-0001](docs/ADR-0001-LANGUAGE.md)。当前 v0.5 产品主线固定为 Rust
1.96.0；已经实机验证的 Python/BlueZ 实现继续作为行为基准和回退，直到 Rust 通过
相同硬件验收门槛。

开发顺序固定为 Ubuntu 优先：先在 Ubuntu 22.04 完成并验收 v0.5，再把仓库迁移到
Windows 11 开始第二阶段适配。Windows 支持目前是计划，不是已经验证的兼容性声明。
详见[项目目标](docs/PROJECT_GOAL.md)与[平台架构](docs/CROSS_PLATFORM.md)。
完整产品范围、使用方式和验收条件单独维护在
[中文需求规格](docs/REQUIREMENTS.zh-CN.md)中；开发规则不能代替需求文档。

模块化 Rust v0.5 主线现在可以编译成一个 Ubuntu 可执行文件，并已通过受控原生生命周期
checkpoint：只读路径精确匹配两条私有 member-ID，设备报告预期双成员配置 ready；实机
STOP 对照仍证明该配置不是实时 linked 状态。Rust 原生 `start/stop` 已获接受，两轮无
按键冷 `start` 第一轮曾报告 `br_edr`，第二轮最终为 `le`。已配对且 trusted 的稳定
public 对象只负责触发 BlueZ 连接；程序只在唯一 connected random GATT 对象的 FDDF
精确匹配 PID 与内嵌稳定身份后才采用。显式恢复回到 `ready`，持久 session 的两次正常
`stop` 约为 0.44 与 0.57 秒。

约 `03:45` 的早期 Rust 完整歌曲尝试不能作为协议结论：同一实验窗口内 Home Centre
自动发送了 STOP，JBL 单响结果受到并发写入污染。后来对不含 `7957` 的 Home-flow-only
build 做干净对照：EOF 修复后 Aura AA ON 与 JBL Wi-Fi ENTER accepted/local linked，
等待 `15` 秒再仅向 JBL 网络播放，用户仍确认 JBL 响、琉璃不响。这否定了目标方向的
无 `7957` 设计，也排除原 `2` 秒过短是唯一原因；固定 `10.5`/`15` 秒不是已证明修复。
随后 exact-GATT 候选重新纳入独立 Assistant `7957` 的 JBL broadcaster 语义。手机
控制退出后 START accepted，MA 仅向 JBL 网络以请求的 `5%` 播放，用户明确确认两台都
响，这是首轮 Rust 目标方向声学通过。HTTPS `7957` 虽 HTTP 200 但设备返回 unknown
command；GATT `0x002a` 仅有 ACK、无 `7951`。普通 STOP 因 Aura ACK timeout 进入
outcome-unknown，显式 recover-stop 在 `13` 秒内 accepted/ready。安装 fresh-bearer
release 修复后，第二轮重启服务并退出手机蓝牙控制，再次在仅 JBL、请求 `5%` 下确认
双响；音乐 idle 后普通 STOP 约 `43` 秒 accepted/ready，无需 recovery。按约定两轮成功
后停止声音测试。P0/发布仍未完成：`7951` 未确认，且曾有深待机需要手机自动连接唤醒。
最新 no-button cold run 已验证整体 `fresh_le` fallback，但未单独命中 A2DP
`wake_then_stable` 分支。该实现仍是跨两个官方状态机的方向化组合，不伪称同一 UI 序列。
旧 v0.4 双响记录仍是独立证据。v0.4 也继续作为需要人工明确选择的回退，Rust 遇到
写前拒绝或结果不确定时不会自动切换后端。两个版本共享 owner-only 的
operation/session 锁，不能同时占有音响。Rust 日常页面使用本机 `8096`，与 Music
Assistant 的 `8095` 分离。见
[Rust 实现说明](rust/README.md)与
[脱敏 checkpoint 证据](docs/RUST_LAN_EVIDENCE_2026-08-30.md)，以及
[官方 App 与 Rust 对照](docs/OFFICIAL_APP_RUNTIME_EVIDENCE_2026-08-30.md)。

长期产品目标已经扩展为本地优先的开源 JBL One 替代：尽可能覆盖两个最接近公开项目的
有用能力，同时保留本仓库独有的 Play Together 后端。完整目标和逐项真实状态见
[Open JBL One 产品需求](docs/OPEN_JBL_ONE_REQUIREMENTS.zh-CN.md)与
[功能覆盖矩阵](docs/FEATURE_PARITY.zh-CN.md)。建议另建通用主仓库，避免破坏本仓库
v0.4 已经形成的设备对证据历史。

当前实际执行主线不是铺开普通 JBL 控制，而是
[Play Together Rust 实施计划](docs/PLAY_TOGETHER_RUST_PLAN.zh-CN.md)：先完成无 App
start/stop/status、冷启动、恢复、最小页面与 Ubuntu 发布，其他功能全部后置。

## 已测试组合

- JBL Authentics 300，固件 `26.24.31.50.00`
- Harman Kardon Aura Studio 5
- JBL One Android `2.7.9`
- Harman Kardon One Android `2.6.11`
- Ubuntu 22.04 / BlueZ 5.64

其他固件、系统和型号均未验证。

## 历史 v0.4 兼容路径的工作方式

1. 本地轻量管理器从实时 FDDF 广告解析琉璃当前随机 LE 地址（经典地址保留为兼容
   回退），再连接 JBL，并在改变任何角色前持有两条控制会话。
2. v0.4 向 JBL 发送 OneOS ENTER 和 `7957 action=1`；向琉璃 5 发送 AA
   `0x3c=ON`。
3. `stop` 不重新连接，而是沿已持有的会话按“琉璃 OFF → JBL `action=2` → JBL
   EXIT”的安全顺序解除。
4. `stop` 后会话保持 `ready`，以后可直接再次 `start`；`shutdown` 关闭会话，再有界地
   尝试恢复先前由工具释放的琉璃 A2DP。A2DP 恢复失败会被明确报告，但不会继续扣住
   厂商控制会话。

这种方式让两台音响由设备固件协同，而不是让 Linux 同时驱动两个独立时钟的音频
sink；后者在实测中有明显一前一后。

## v0.5 Rust alpha 快速开始

先按文档在仓库外准备 owner-only 私有配置，包括你有权使用的客户端证书/私钥、精确
设备 pin 和私有身份锚点。本项目不分发厂商凭据。启用写入前先阅读完整
[Rust alpha 指南](rust/README.md)。

```bash
./rust/build-neutral-release.sh
./scripts/install-rust-user-service.sh
# 填写并检查安装后的 owner-only 配置权限。
jbl-aura-link shutdown
systemctl --user disable --now jbl-aura-link-session.service
systemctl --user enable jbl-aura-link-rust.service
systemctl --user start jbl-aura-link-rust.service
jbl-aura-link-rust status
jbl-aura-link-rust start
jbl-aura-link-rust stop
```

本机页面为 `http://127.0.0.1:8096`。已测试固件默认
`JBL_BROADCAST_CONFIRMATION=ack`，因此传输接受返回
`accepted_unconfirmed`/`broadcast_acknowledgement_only`，CLI exit `0`；这不等同
`7951` 或声学成功。

## v0.4 回退快速开始

安装小型运行依赖：

```bash
sudo apt install bluez bluez-tools jq python3 python3-venv xxd
# PulseAudio 主机还需：sudo apt install pulseaudio-utils

runtime_env="${XDG_DATA_HOME:-$HOME/.local/share}/jbl-aura-link/venv"
python3 -m venv "${runtime_env}"
"${runtime_env}/bin/pip" install -r requirements-le.txt
```

先用 `bluetoothctl` 配对并信任两台音响。手机和其他主机应断开琉璃 5，但不用删除
配对。然后把示例配置安装到仓库之外：

```bash
config_path="${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link/devices.env"
install -Dm600 config/devices.env.example "${config_path}"
# 替换两条占位蓝牙地址，并把 PYTHON_BIN 指向上面的 venv/bin/python。

./bin/jbl-aura-link doctor
./bin/jbl-aura-link install-service

jbl-aura-link status
jbl-aura-link start
```

用 JBL 已支持的任意方式开始播放，并分别听两台音响。解除关联但保留控制会话：

```bash
jbl-aura-link stop
```

要让 App/其他主机接管或明确结束控制会话时：

```bash
jbl-aura-link shutdown
```

v0.4 默认最多执行 3 段 30 秒实时 FDDF 扫描，中间各等待 15 秒。若三段之后仍处于
广播空窗，启动会在写命令前失败，不会拿缓存随机地址冒险；已安装的 systemd 服务会
再等 20 秒后重试。两轮无按键冷启动已经成功，中间实测的一次首段扫描失败也被新策略
覆盖。只要管理器仍在 `ready/linked`，`start/stop` 不需要重新扫描，仍是日常最可靠
路径。

## 命令

| 命令 | 作用 |
|---|---|
| `doctor` | 检查依赖、配置、适配器与配对 |
| `install-service` | 安装并启用用户级开机服务和简化命令 |
| `start` | 创建或复用持久控制会话，然后关联 |
| `stop` | 通过已持有的会话解除，并保持 `ready` |
| `shutdown` | 解除并关闭两条会话；随后尽力恢复先前的 A2DP |
| `status` | 显示 managed 状态；离线时只给保守的 DFFD 诊断 |
| `recover-stop` | 没有持久会话时显式执行一次尽力恢复 |
| `frame` | 只离线构造 PL 帧，不接触硬件 |

`install-service` 只把公开 CLI、会话管理器与 unit 模板安装到 `~/.local`，并启用
`jbl-aura-link-session.service`。开机后服务只建立控制会话并保持 `ready`，不会自动
关联或播放；之后直接运行 `jbl-aura-link start|stop|status`。真正早于登录启动需要
用户 lingering，安装器会在未开启时给出提示。没有安装固定 unit 时，`start` 仍保留
临时用户级 systemd 回退。持久会话本身使用 Python 标准库；自动 LE 冷发现额外使用
小型 `dbus-fast` 依赖。管理器通过权限为 `0600` 的本地 Unix socket 接收命令；
runtime/state 目录权限为 `0700`。
daemon 会持续监测两条控制 bearer；空闲断线或命令进入 `degraded` 时主动非零退出，
由固定 unit 重新扫描并连接。若上一状态可能改变过音响角色，新 daemon 会先发送已验证
的 OFF/STOP/EXIT 归一化，再发布 `ready`。
默认 `auto` 兼容模式只在建立两条会话时临时卸载 PulseAudio 的
`module-bluetooth-policy` 和 `module-bluetooth-discover`，随后立即恢复它们。控制
会话在模块恢复后仍保持连接。权限为 `0600` 的私有恢复快照保证 systemd 启动失败或
重试后也能补回模块。这几秒内其他蓝牙音频会短暂中断。

对已经验证 FDDF/LE 的硬件，开机服务建议设置 `AURA_TRANSPORT=le`。`auto` 会保留经典
兼容回退，而某些 BlueZ 主机可能因此同时生成琉璃 A2DP Sink。

如果管理器在 `linked` 时被强杀、主机掉电或音响掉电，就不能保证还可新建连接来
自动解除。`recover-stop` 因而明确标为 best-effort；它不会在缺少确认时谎报成功。

真实配置、状态和日志默认都在仓库之外。不要把蓝牙地址、家庭网络地址、抓包、证书、
token、APK、固件或账号材料提交到仓库或公开 issue。

## 文档

- [复现步骤](docs/REPRODUCTION.md)
- [无按键冷重连验收记录](docs/COLD_RECONNECT_2026-08-29.md)
- [协议记录](docs/PROTOCOL.md)
- [成功证据与未决问题](docs/EVIDENCE.md)
- [官方 App 动态运行证据（2026-08-30）](docs/OFFICIAL_APP_RUNTIME_EVIDENCE_2026-08-30.md)
- [已有开源研究](docs/PRIOR_RESEARCH.md)
- [安全与隐私](SECURITY.md)
- [版本记录](CHANGELOG.md)

## 资源占用

实现仅包含 Bash、Python、`dbus-fast` 和 BlueZ `gatttool`。它不依赖模型、音频
解码器、云服务或账号；真实回归中的常驻管理器约占十余 MB 内存。

原创代码和文档采用 MIT 许可证。本项目与 JBL、Harman Kardon 及其所有者无隶属或
背书关系；产品名仅用于说明兼容性。公开仓库不分发 APK、固件、反编译源码、账号
材料或厂商秘密。
