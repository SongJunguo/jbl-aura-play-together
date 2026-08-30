# Play Together Rust 主线实施计划

- 建立日期：2026-08-30
- 当前平台：Ubuntu 22.04
- 固定设备：JBL Authentics 300 + Harman Kardon Aura Studio 5
- 当前状态：研究冻结，未发布
- 最高优先级：Play Together P0

## 0. 当前决策：停止盲测，转入双端逆向

2026-08-30 最新持续听感已经否定“继续调整命令顺序或固定等待时间”的开发方式：

- compatibility JBL-source 的控制 ACK accepted，但恢复 `20%` 播放后持续只有 JBL；
- 切换为单一 Aura-source 后，最初疑似双响没有持续，十几秒后只有 Aura；
- 两次结果都不能由 `linked`、`healthy`、双成员配置、AA ACK 或 `7957` ATT ACK 解释为
  成功；未验证的 official profile 已撤回，不能设为默认或发布。

主线因此冻结为以下研究顺序，任一未知项都禁止猜测：

1. **App 控制端还原**：精确还原 JBL One `2.7.9` 中“先点 Aura、再点底部 JBL”第二步
   的 UI → ViewModel → runtime device class → session/transport → serializer → callback →
   reducer 全链；JADX 失败处必须用原始 DEX/Smali/Androguard 或另一反编译器交叉确认。
2. **固件被控端还原**：解析 Aura Studio 5 精确 OTA 的 AA `0x3c` handler；取得并解析
   Authentics 300 精确固件，定位 `7937/7938/7942/7951/7955/7957`、action `31..34`、
   source election、receiver bind 与业务状态机的真实前置条件。
3. **USB 动态闭环**：静态或固件仍不能消除的分支，只允许用现有手机临时 USB ADB
   复现一次官方完整 UI 流程；关联 UI hierarchy、logcat、HCI、网络时序，必要时在加密前
   做授权的方法级 hook。无线调试端口不再作为主证据通道，最终运行不依赖手机。
4. **Clean-room 实现**：只有控制端与被控端证据一致后，才生成脱敏 fixture、更新 Rust
   状态机并完成全部离线失败矩阵。不得复制厂商表达性源码、APK、固件或凭据。
5. **唯一实机验收**：全部证据门槛与离线测试通过后，只执行一次 `20%` 音量的受控
   start → 播放 → 持续听感 → stop。缺少业务后置条件时不得开始；结果失败即返回研究，
   不连续换顺序、加延迟或重试。

当前第一阻断项是：**底部 JBL 第二次点击的完整事务仍未知**。当前第二阻断项是：
**Authentics 300 精确固件及其命令分发表尚未取得**。在两项关闭前，Rust 直接控制只保留
历史/实验身份，不再宣称日常可靠。

详细任务、证据表与停止条件由本仓库的公开计划和证据文档自洽维护；私有研究工件不进入
本仓库，也不记录私人路径、设备标识或原始反编译材料。

## 1. 最终目标

在 Ubuntu 22.04 上完成一个低资源、隐私安全、Rust-first 的 Play Together 控制器，
实现：

- 日常无需手机 App 的 `start / stop / status`；
- 不反复按蓝牙键的冷启动；
- JBL 设备报告的双成员配置与私有成员身份验真，并与实时 managed 状态分离；
- FDDF 广播空窗、手机占用、网络/音响中断和进程重启的有界恢复；
- 单写者状态机、开机服务和最小本地 Web UI；
- 一个面向用户的 Ubuntu 可执行文件；
- Rust 尚未等价前保留 v0.4 Python/BlueZ 回退。

EQ、通用播放、源、按键、模式和 Home Assistant 等 JBL One 普通功能暂停，只记录需求，
不抢占本计划。

## 2. 当前基线

### 已经实证

- v0.4 Python/BlueZ 后端已完成真实 Play Together 双响；
- 持久会话已连续完成多轮 start/stop；
- 无按键 FDDF 冷发现已有两轮成功，也记录过一次广播空窗安全失败；
- 当前 JBL API 可报告 JBL + Aura 两成员配置；实机 STOP 对照证明它不会随 live 状态
  消失；
- Rust 1.96.0 已固定；
- Rust alpha 已使用运行时 mTLS 身份和设备证书指纹，只读取得 Authentics 300 信息与
  精确私有 member-ID 验真的双成员配置；
- Rust 离线模型、配置、TLS、组解析、整对状态机、Web 与服务测试已通过；
- Rust 原生 `start/stop` 已获真实设备接受；显式恢复已沿“安全诊断 → 已配对稳定身份
  → 精确 FDDF 随机 GATT 身份”恢复到 `ready`；
- Rust 已完成两轮无按键冷 `start`：第一轮 managed 状态曾报告 `br_edr`，第二轮最终
  `le`；持久会话两次正常 `stop` 约为 0.44 与 0.57 秒；
- 写前日志已实证跨进程崩溃保留 pending，并阻止未经确认的后续普通写入；
- Rust 日常服务与 Web UI 使用本机 `8096`，Music Assistant 保留独立 `8095`。

### 仍未完成或仍需冻结

- 已通过只读能力、端口与 App 路由交叉检查确定：本机 8080 拒绝连接且未声明
  `websocket_connect`，JBL 主通道应为 mTLS HTTPS 独立
  `enterAuracast/exitAuracast`；
- `sendAppController` 是按键模型，不作为 Play Together 命令载体；
- 约 `03:45` 的完整歌曲尝试与 Home Centre 自动 STOP 重叠，不能用于协议结论；后来
  的 EOF-fixed clean Rust Home-flow-only 事务不含 `7957`，Aura AA ON 与 JBL Wi-Fi
  ENTER accepted/local linked，等待 `15` 秒再仅向 JBL 网络播放仍只有 JBL 出声；
  该结果否定目标方向的无 `7957` 设计，不能用固定 `10.5`/`15` 秒掩盖；
- exact GATT `0x002a` `7957` 候选 START accepted，MA 仅向 JBL 网络以请求的 `5%`
  播放，用户确认两台都响；普通 STOP 因 `aura_ack_timeout` outcome-unknown，显式
  recover-stop 在 `13` 秒内 accepted/ready；安装 fresh-bearer release 后第二轮再次
  START 双响，音乐 idle 后普通 STOP 约 `43` 秒 accepted/ready、无需 recovery；按约定
  停止声音测试，但 P0/发布、`7951` 与 A2DP wake 子路径仍未完成；
- 窄 UFW callback 规则安装后的 production strict GENA START 仍
  `jbl_broadcast_result_timed_out`，随后 legacy GATT 归一化；规则不是协议成功；
- HCI 已冻结深待机唤醒顺序：Android BR/EDR A2DP auto reconnect → stored link-key
  auth/encryption → AVDTP Open → 约 `2.5` 秒后 FDDF → 更晚的 App LE 读取；wake module
  已 production 接入并进入 neutral artifact；默认冷链路为 stable raw 一次 → eligible
  failure 时单次 A2DP ConnectProfile（`20` 秒）→ fresh FDDF exact gate（`30` 秒）→
  DisconnectProfile 并确认释放（`5` 秒）→ stable raw retry → 原 LE fallback，共享
  `150` 秒 outer deadline；释放未确认即写前失败；最新无声硬件轮次经 `fresh_le` 在
  `150` 秒内完成，整体 no-button cold 通过，但 A2DP `wake_then_stable` 未命中/证明；
- 当前 `258` lib + `8` CLI（主 harness `266`）及 FIFO private-file helper `1/1`，
  audit/deny/fallback/privacy/neutral 全绿，compat evidence mode 完成；
- 手机占用、断网、音响关机等剩余失败矩阵仍需补齐；最终资源/产物已冻结；
- Rust Stage A 本地安全门禁、最终测试总数、ELF 大小/ABI/依赖与 idle 资源样本已记录，
  不沿用更早中间检查点数字；
- 标准 BASS/BASE/BIG/BIS/ISO 数据面仍未证明。

## 3. 实施原则

1. 先只读、后写入；先合成测试、后实机。
2. 每次硬件写入都必须有前置快照、预期后置条件、超时和回退方案。
3. 设备报告双成员配置用于排除错误设备，但不是 live 成功门槛；START/STOP 还要求对应
   应用层与 Aura ACK、健康单写者会话，发布验收另需人工双响。任一证据不能冒充另一种。
4. 已有健康组优先采用，不为测试无意义拆建。
5. v0.4 回退在 Rust 通过同等失败场景前不得删除。
6. 只允许一个控制写入者；CLI、服务和以后 Web UI 共用同一状态机。
7. 所有重试有次数和时间上限，不做无限扫描/连接/重启。
8. 实机声学测试保持低音量，除非用户明确要求，否则程序不改音量。
9. 私钥、证书、IP、MAC、指纹、组/member ID 与原始响应永不进入公开历史或产物。
10. 完成两轮冷启动后不继续为“刷次数”做破坏性重复测试。

## 4. 阶段 A：冻结基线与修复 Rust 只读核心

### 工作

- 用 Rust 与现有 Python 同时读取同一设备状态，做脱敏差分；
- 强制确认目标型号为 Authentics 300；
- 组配置验证要求 `disabled == false`，成员 ID 精确匹配私有 JBL/Aura 身份，并按固定
  型号角色投影；未知名称和 ID 不原样回显；
- 禁止 HTTPS 重定向、启用 HTTPS-only 与总请求超时；
- 限制配置、证书和密钥为常规文件、当前用户所有、无符号链接、有限大小；
- 移除 `RuntimeConfig` 的敏感 Debug；
- 通过固定中性构建视图消除 release ELF 中的构建机私人/临时路径；
- CI 增加 Ubuntu 22.04、全历史 checkout、锁定 Rust 和发布产物扫描；
- 隐私扫描覆盖二进制、证书容器、十六进制标识和完整 Git 历史，失败日志不回显秘密。

### 通过门槛

- Rust status/group 与 Python 对同一实机给出一致、脱敏的核心结论；
- 错误 pin、错误型号、缺失 disabled、错误权限和超限响应均安全失败；
- fmt、Clippy、锁定测试、release 构建、隐私扫描全部通过；
- 本阶段不改变音响状态。

2026-08-30 本机 Stage A 已达到以上门槛：Rust/Python 同刻脱敏状态一致，RustSec、
许可证与来源策略通过，全历史隐私扫描通过，固定中性路径的离线 release 及产物扫描
曾在该 checkpoint 通过。加入修订事务后，最终测试总数、neutral ELF
已重新冻结：`258` lib + `8` CLI（主 harness `266`）、FIFO private-file helper `1/1`，
artifact `8,284,440` bytes、`GLIBC_2.34`、仅
`libc`/`libgcc`，安装 hash 一致；具体 SHA 仅留 release 内部。Stage A 全过程未发设备
写命令。

## 5. 阶段 B：确定 JBL 主控制通道

主通道已按只读证据选定，实施顺序如下：

1. 保留 `enterAuracast`、`exitAuracast` 与 BasicResponse fixture；单独 Assistant
   `setAuracastBroadcast` action 1/2 继续与 Home UI 状态机分开建模；
2. 写入 Agent 禁止连接池复用、禁止重定向，`error_code==0` 只表示命令被接受；
3. 保留已验证的 JBL GATT PL `7937/7938` 作为回退；
4. WS 8080 仅在未来固件明确声明能力时再实现，当前不作为阻断项；
5. 动态结果已证明本固件 HTTPS `7957` 在 pin 匹配后仍返回 HTTP 200/unknown command；
   目标方向使用 exact GATT handle `0x002a` 的 `7957`，记录独立 ATT ACK、managed 状态
   与声学结果；
6. 若 LAN 通道不能得到应用层结果或 live 结果未知，回退到 v0.4 已验证路径，不强行宣布成功。

2026-08-30 约 `03:45` 的旧尝试被 Home Centre 自动 STOP 污染，不能据此判断协议。
后来的 simultaneous capture 交叉确认官方 Home flow 为 Aura-source：Android A2DP
进入 Aura，播放后 Aura 为 PRIMARY、JBL 为 RECEIVER；JBL 控制是 Wi-Fi
`enterAuracast/exitAuracast`，不含 `7957`。EOF 修复后的 clean Rust 复现了该 Home
控制面，accepted/local linked 后等待 `15` 秒再仅向 JBL 网络播放，用户仍确认 JBL 响、
琉璃不响。这否定了把 Aura-source Home flow 直接用于 JBL-source 目标的设计，也排除
原 `2` 秒过短是唯一原因。目标方向现重新纳入单独 Assistant `7957` 的 JBL broadcaster
语义，并与 Aura AA receiver 语义做跨两个官方状态机的方向化组合；不伪称同一 UI
序列。这里的“回退”仍只表示操作者可以另行选择 v0.4；不自动切后端或重发。

exact GATT `7957` 候选随后完成首轮 START 声学 gate：手机 App/蓝牙控制退出后 START
accepted，MA 仅向 JBL 网络以请求的 `5%` 播放，用户明确确认两台都响。四次 JBL GATT
写均有 ATT ACK，但没有 `7951`；GENA callback 未出现，后续窄 UFW 规则下的 strict
试验仍 timeout，不能再把“规则已安装”当作修复。
普通 STOP 因 Aura ACK timeout 进入 outcome-unknown；显式 recover-stop 在 `13` 秒内
accepted/ready。安装 fresh-bearer release 后，第二轮重启 service、退出手机蓝牙控制，
START accepted，MA 仍仅向 JBL 网络以请求的 `5%` 播放，用户再次确认双响；音乐 idle
后普通 STOP 约 `43` 秒 accepted/ready，无需 recover。按用户约定两轮成功后停止声音
测试。声学 gate 已完成，但 `7951` 未确认，且曾有深待机需要手机自动连接唤醒；P0/
发布仍未完成。

strict GENA 在用户授权安装窄 UFW callback 规则后仍以
`jbl_broadcast_result_timed_out` 失败，随后 legacy GATT 归一化。当前固件仍无实证
`7951`。配置闭集为 `JBL_BROADCAST_CONFIRMATION=ack|gena`，默认 `ack`：ACK 返回
`accepted_unconfirmed`、`broadcast_acknowledgement_only` 且 CLI exit `0`；只有 GENA
动作 `33/34` 返回 `accepted`、`broadcast_business_notification`。managed `linked` 不
等同 `7951` 或声学成功。

2026-08-30 native false-idempotent 实机反例中，status 为 managed linked、
health/lifecycle healthy/linked、Aura transport `le`、route `fresh_le`，但 Aura 无声；
START 幂等返回且没有设备写，只有 JBL 响。纠正后真实 STOP `46.71` 秒返回
`accepted_unconfirmed`；首次真实 START `49.76` 秒写前拒绝、journal clean；单次有界
retry `48.56` 秒返回 `accepted_unconfirmed`。
随后测试播放器 idle 导致两台都无声，不能判为组网失败；已直接恢复网络音源 `20%` 播放，
JBL `state=playing`、`volume=20` 后用户确认仍只有 JBL 响、Aura 无声。因此本轮 ACK-only
accepted START 声学失败；不推翻历史两轮双响，只证明 compatibility transaction 不稳定。

随后停止 JBL 网络音源并启动已有 Aura A2DP bridge。测试播放器状态为 JBL
idle/volume `20`、Aura playing/volume `20`。最初“两台都响”没有持续；继续听十几秒后
用户更正为 Aura 响、JBL 不响。该瞬态只记为疑似残余缓冲，不算声学通过，也不能推出
`audio-sync` 需求。随后停止 Aura 测试音源与 A2DP bridge，最终两个 player 均 idle、
bridge exited。本轮两个方向都未持续双响，不建立新的实用方案。

深待机 HCI 顺序为 Android 系统 BR/EDR A2DP 自动重连、stored link-key 鉴权/加密、
AVDTP Open，约 `2.5` 秒后 FDDF，App LE 读取更晚。wake module 已 production 接入：
stable raw once → 单次有界 A2DP wake → `30` 秒 fresh FDDF exact identity/PID → `5` 秒
确认 profile release → stable raw retry → 原 LE fallback，全程共享 `150` 秒 deadline。
release 未确认即不发送角色写。neutral artifact 与离线门禁已通过。最新无声硬件轮次
START `122.15` 秒 accepted_unconfirmed/linked，status 双成员 verified/healthy、route
`fresh_le`，STOP `15.89` 秒 accepted_unconfirmed/ready，journal clean、`NRestarts=0`。
手机 App 未参与本轮，但无 ADB 手机状态证据。整体 no-button cold 已实机通过；A2DP
`wake_then_stable` 子路径未单独命中/证明。

### 选择标准

- 首选：局域网通道，有明确回复，不占用 JBL 蓝牙；成员配置只作为身份前置条件；
- 回退：持久 JBL GATT 会话；
- 禁止：只得到 HTTP 200/写 ACK 就称作完成。

## 6. 阶段 C：Aura Rust 控制后端

### 先实现接口，再替换后端

统一接口至少包括：

```text
discover_verified_identity
connect_and_hold
set_play_together_on
set_play_together_off
health
shutdown
```

### 迁移顺序

1. Rust 状态机先允许调用现有 v0.4 持久会话作为兼容后端；
2. 原生 Ubuntu 后端优先直接使用 zbus/BlueZ D-Bus；`bluer` 作为工作量过大时的
   备用，raw ATT 只作 D-Bus 路径失败后的逃生口；
3. 将 FDDF UUID、PID、内嵌稳定身份和实时 RPA 匹配移入 Rust；
4. 按已确认的 vendor service、write、notify UUID 唯一发现 characteristic；
5. 实现持久 GATT/ATT 会话、先订阅通知再写入；
6. ON/OFF 必须收到精确 AA 成功回复；
7. 测试 FDDF 空窗、手机占用、断线和重启；
8. Rust 原生后端达到等价后，兼容后端仍保留一个发布周期。

不把固定随机 LE 地址、同名设备或旧 BlueZ 缓存当作身份。

2026-08-30 原生 FDDF/GATT 与成对后端已完成实机 checkpoint。直接连接已发现的 LE
`Device1` 失败后，已配对且 trusted 的稳定 public 对象可以触发 BlueZ 连接，但 vendor
GATT 实际挂在唯一 connected random 对象；只有该对象的 FDDF 精确匹配 PID 与内嵌稳定
身份才采用。两轮冷启动第一轮 managed 状态曾报告 `br_edr`，第二轮最终 `le`；显式恢复
也沿上述身份门完成并回到 `ready`。v0.4 仍单独保留，但不自动 failover。

## 7. 阶段 D：单写者 Play Together 状态机

状态集合：

```text
offline -> ready -> linking -> linked
                     |          |
                     v          v
                  degraded <- unlinking
                     |
                  recovering
```

### `status`

- 读取 JBL LAN 成员配置；
- 合并 JBL/Aura 控制通道健康；
- 区分 App 已建组、程序已建组和本地会话状态；
- 明确区分 `pair_configuration=ready`、本程序本轮 managed live 状态和外部状态 unknown；
- 不把 Music Assistant 是否播放混入组状态。

### `start`

1. 获取跨进程独占锁；
2. 读取前置成员配置并验证私有身份；
3. `NativePair` 不允许 START 幂等快路：即使 managed linked、health
   healthy/lifecycle linked、成员 verified 且 Aura route 已解析，也必须执行 backend；
   仅 legacy held session 可保留同 session 幂等；
4. 建立/确认 Aura 控制会话；
5. 执行选定的 JBL ENTER 路径；
6. 通过 exact GATT value handle `0x002a` 请求 JBL `7957` broadcaster 语义；ATT ACK
   只算传输接受，不能冒充 `7951` 或声学成功；
7. 执行 Aura ON 的 AA ATT Write Command，并验证相关业务通知；
8. 复核成员配置未指向错误设备，并把 ACK 后的本轮 managed live 状态记录为 linked；
9. 失败进入 degraded，记录安全错误并保留回退条件。

`ack` 模式的 `accepted_unconfirmed` 允许 CLI exit `0`，但必须携带
`broadcast_acknowledgement_only`；严格 `gena` 只有动作 `33/34` 才是
`broadcast_business_notification`。两种模式都禁止把 managed `linked` 冒充声学证明。

这是针对“网络音频进入 JBL”的方向化组合，跨 Assistant broadcaster 与 Home/AA
receiver 两个官方状态机。两轮 START 声学已通过，fresh-bearer 修复后有一轮普通 STOP
通过；仍需冻结更深待机、业务确认和发布门槛。不能声称是官方 PartyTogether 页面的
一次原样事务，也不能把固定等待当协议步骤。

### `stop`

1. 获取同一锁；
2. 读取前置成员配置与 managed live 状态；
3. `NativePair` 不允许 STOP 幂等快路；managed ready/healthy 不能替代设备角色写；
4. Aura OFF 并验证回复；
5. 在独立 Assistant 状态机中请求停止 JBL broadcaster，再执行 JBL EXIT；
6. 不等待双成员配置消失；成功 ACK 后记录 ready，bearer/外部动作不确定时记录 unknown；
7. 默认保留最可靠的 ready 控制会话；
8. `shutdown` 才释放会话给 App/其他主机；managed offline 的重复 shutdown 可本地 no-op。

### `recover`

- 普通 start/stop 不自动升级为破坏性恢复；
- 连续失败达到阈值才建议恢复；
- CLI/Web UI 需显式确认；
- 恢复仍有总时间与次数上限。

每次可变操作前，状态机先把不含身份的 pending 记录持久化。仅在操作被接受且重新读取
的成员配置仍精确指向预期设备对后清为 clean；结果未知或进程崩溃时，普通写入保持
闭锁。旧 timeout 构造曾在真实运行中触发 panic，但 pending 跨崩溃保留；timeout 已修，
该 pending 最终只由显式恢复清除。另一次真实 FDDF 空窗在首次设备写前拒绝，因而可以
安全清回 clean。

## 8. 阶段 E：离线与受控实机验收

### 离线矩阵

- legacy held session 的同状态 START/STOP 可幂等采用；
- NativePair 即使 managed/health/lifecycle/route 看似一致也必须重做 START/STOP；
- managed offline 的重复 shutdown 保持本地 no-op；
- 单步失败与回滚；
- 状态过期与并发请求；
- LAN 超时、Bluetooth 断开、FDDF 空窗、错误身份；
- 守护进程崩溃与不确定状态重启；
- 日志/JSON/产物无私人标识。

### 实机顺序

1. 只读记录当前双成员配置、managed 状态和服务状态；
2. 在 v0.4 回退可用时执行一次受控 stop；
3. 验证 receiver-first ACK 与本地 ready；不把仍保留的成员配置误判为 STOP 失败；
4. 执行一次 Rust start；**已得到接受**；
5. 验证成员身份配置、完整控制 ACK 与 managed linked 状态；**已完成 checkpoint**；
6. 用户以低音量确认两台都出声；**`03:45` 尝试已因自动 STOP 作废；EOF-fixed clean
   Home-flow-only 事务无 `7957`，accepted/local linked 后等 `15` 秒仍仅 JBL 出声；
   exact GATT `7957` 首轮以请求的 `5%` 双响通过**；
7. 完成总计两轮连续 start/stop；**两轮 START 双响已通过；第一轮 STOP 需 recovery，
   fresh-bearer 修复后第二轮普通 STOP 约 `43` 秒 accepted/ready；按约定停止声学测试，
   其余非声学/发布门槛继续**；
8. 完成两轮 shutdown 后无按键冷启动；**已完成两轮，managed 状态第一轮曾报
   `br_edr`、第二轮最终 `le`，均未绕过随机对象身份门**；
9. 覆盖一次 FDDF 广播空窗和稍后恢复；**已完成写前安全拒绝、classic nudge 后恢复**；
10. 覆盖手机占用、断网、音响关机和程序重启；**崩溃/pending 已完成，其余待补**。

任一步结果不确定时停止该轮，保留日志和前置状态，不连续盲试。

## 9. 阶段 F：最小 Web UI 与服务

核心通过后才实现：

- `serve` 默认只监听 `127.0.0.1:8096`；Music Assistant 的 `8095` 不被接管；
- 页面/status 显示 Play Together 状态、两个脱敏成员、allowlisted channel、
  `last_action`/`age_ms` 与安全错误；
- 只提供刷新、start、stop；recover 放在独立确认区；
- Controller actor 是唯一写入者；CLI 在服务运行时调用本地 API；
- Host/Origin/CSRF、请求体限制、安全响应头和 revision 冲突保护；
- 用户级 systemd、独占锁、优雅退出和有界重启；
- 静态页面资源编译进一个 Rust 可执行文件；
- 重新测量体积、RSS、CPU 与启动时间。

上述最小页面、同进程 Controller actor、CLI 本地客户端和用户级 systemd 边界已经实现；
loopback 页面可读，CSP/CSRF 保持。新 artifact/installed/process hash 一致；user
systemd 重启后 enabled+active、`NRestarts=0`，一次只读 status 后 managed
unknown/offline。`15` 秒 restart-idle 样本为 RSS `8,828 KiB`、`1` thread、`15` fds、
平均 CPU `0.0667%`（`1` tick）。真实硬件
`start/stop/recover` 已通过服务执行；声学按两轮约定停止，无声 no-button cold 已经
`fresh_le` 通过。剩余为 A2DP `wake_then_stable` 子路径、`7951` 及发布验收，不再等待
资源/产物冻结。

## 10. 阶段 G：发布与后续

### 发布前

- Ubuntu 22.04 构建与运行验收；
- 第三方许可证清单、NOTICE/SBOM、依赖锁；
- release ELF 扫描无凭据、真实标识和构建机私人路径；
- onboarding 明确为 BYO authorized identity，不分发厂商/上游私钥；
- 文档区分 Rust 当前能力、v0.4 回退与尚未证明的 BASS/ISO。

### 仓库策略

- 当前仓库继续保存固定设备对的协议、证据和 Play Together 后端；
- 更广泛的 Open JBL One 主仓库与普通功能在 P0 稳定后再建立/迁移；
- 当前不得为了通用功能推迟 Play Together 发布。

### P0 之后

才开始音量、EQ、播放、源、按键、模式、Home Assistant 和其他型号；Ubuntu 稳定后再
迁移到 Windows 11。

## 11. 完成定义

只有同时满足以下条件，Play Together P0 才可标记完成：

- Rust CLI 可真实 start/stop/status，不依赖手机 App；
- 两轮无按键冷启动和两轮连续 start/stop 通过；当前两轮 START 声学通过且有一轮
  post-fix 普通 STOP 通过；production no-button cold 已经 `fresh_le` 实机通过，A2DP
  `wake_then_stable` 子路径尚未命中；
- 用户确认双响；
- 成员配置、JBL/Aura 回复、managed live 状态、声学结果和错误恢复证据分级完整；
- FDDF 空窗、手机占用、断网、关机、重启均安全有界；
- 最小 Web UI 与 systemd 通过；
- 资源、隐私、许可证、Ubuntu 22.04 发布验收通过；
- 仍不伪称已经证明标准 BASS/BIG/BIS/ISO 数据面。
- `7951` 或其他更强业务状态、A2DP `wake_then_stable` 子路径与最终发布证据仍须完成。
