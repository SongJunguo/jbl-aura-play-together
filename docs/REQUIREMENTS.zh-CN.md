# JBL Authentics 300 + 琉璃 5 控制器需求规格

- 状态：v0.5 开发基准
- 建立日期：2026-08-30
- 当前实施平台：Ubuntu 22.04
- 后续平台：Windows 11（Ubuntu 完成后迁移仓库继续开发）

本文档定义“最终要做成什么”。`AGENTS.md` 定义开发与发布时必须遵守的规则；两者用途
不同。需求发生变化时，应先修改本文档，再修改实现。

## 1. 产品目标

为以下固定设备组合提供可靠、低资源、无需日常依赖手机 App 的本地控制器：

- JBL Authentics 300
- Harman Kardon Aura Studio 5（琉璃 5）

控制器应能够建立、解除、检查并恢复两台音响的厂商 Play Together 关联。正常使用时，
用户只需一个程序和简单页面或命令，不应反复按音响蓝牙键、重新配对或打开 JBL/Harman
手机 App。

优先目标是“状态真实、操作可靠、容易安装”，不是增加大量未经验证的功能。

Play Together 是当前 P0 独有核心。更广泛的 JBL One 替代功能属于 P2；在本需求第 7
节的 Ubuntu Play Together 验收门槛完成前，不并行实现 EQ、通用播放、源、按键或模式。

## 2. 交付顺序

### 2.1 第一阶段：Ubuntu 22.04

先在当前 Ubuntu 主机上完成、实测并发布。Windows 工作不得分散这一阶段的开发资源，
但协议模型和接口不能故意绑定 Linux。

Ubuntu 目标交付物：

1. 一个面向用户的 Rust 可执行文件；
2. 模块化源码和锁定依赖；
3. 仓库外的私有配置与凭据目录；
4. CLI：`doctor`、`start`、`stop`、`status`、`serve`，以及受控恢复入口；
5. 程序内置的本地 Web UI；
6. 可选的用户级 systemd 开机服务；
7. 离线测试、隐私扫描和真实硬件验收记录；
8. 在 Rust 未达到功能等价前，保留现有 Python/BlueZ 路线作为参考和回退。

### 2.2 第二阶段：Windows 11

Ubuntu 版本通过验收后，由用户把仓库迁移到 Windows 11，再继续：

1. 复用同一协议模型、脱敏 fixture 和证据状态；
2. 验证相同的 JBL LAN/mTLS 客户端；
3. 用 WinRT/Windows BLE 后端替代 Ubuntu 专用的 BlueZ/gatttool 调用；
4. 使用 Windows ACL 保护凭据；
5. 增加 Windows Service 或任务计划支持；
6. 构建独立的 Windows `.exe`；
7. 重新完成成员配置、managed live 状态与人工双响的分级验收。

Windows 支持在完成上述验收前只能标记为“计划中”或“实验性”。

## 3. 用户使用方式

### 3.1 单文件含义

用户每个平台只需要拿到一个主程序：

```text
Ubuntu: jbl-aura-link
Windows: jbl-aura-link.exe
```

“单文件”指一个发布可执行文件，不是把全部源码写进一个巨型文件。协议、LAN、蓝牙、
状态机、配置、CLI 和 Web UI 在源码中必须保持模块化，最后编译为一个程序。

证书、私钥和设备配置不得嵌入可执行文件，仍从操作系统的私有配置目录加载。

### 3.2 CLI

目标命令：

| 命令 | 目标行为 |
|---|---|
| `doctor` | 只读检查配置、权限、网路、蓝牙适配器、设备发现和能力 |
| `status` | 显示真实组状态、成员、连接状态、最近动作与错误 |
| `start` | 建立或采用 JBL + 琉璃 5 的 Play Together 组 |
| `stop` | 解除组，但保留最可靠的后续重连条件 |
| `serve` | 启动内置本地 Web UI 与受限 API |
| `shutdown` | 明确释放控制会话，供 App/其他主机接管 |
| `recover` | 仅在普通路径失败后执行有界恢复，不自动无限重试 |

当前 Rust alpha 已实现独立的 `jbl-aura-link-rust` 服务、`start`、`stop`、`status`、
`serve`、`doctor`、`group` 与需要精确确认的 `recover-stop`。它已经通过受控实机
生命周期 checkpoint，但尚未满足本节后文的全部稳定版门槛；发布时是否收敛为最终
`jbl-aura-link` 名称在冻结阶段决定。

### 3.3 Web UI

运行：

```text
jbl-aura-link serve
```

默认仅监听：

```text
http://127.0.0.1:8096
```

`8096` 是 Rust 日常控制端口；Music Assistant 继续使用独立的 `8095`，控制器不得接管
或代理其页面。

HTML、CSS 和少量 JavaScript 已作为静态资源编译进 Rust 程序，因此不增加额外部署
文件。当前 loopback Web/status 已可读，并显示：

- JBL LAN 是否可达；
- 琉璃控制通道状态；
- 设备报告的预期双成员配置是否存在；
- 当前实时状态证据是本程序本轮已关联、已解除，还是外部状态未知；
- 两个脱敏成员与 allowlisted 声道；
- 当前管理状态：离线、就绪、关联、退化、恢复中；
- 最近一次动作、`last_action`/`age_ms`、结果和安全错误摘要；
- `启动关联`、`解除关联`、`刷新状态`；
- 恢复操作必须单独确认，不能与普通启动按钮混淆。

默认页面不得显示 IP、MAC、证书指纹、组 ID、成员 ID、私钥路径或原始 JSON。
CSP、Host/Origin 与 CSRF 防护必须保持启用；最终冻结页面已验证满足该要求。

手机访问不属于第一版默认行为。以后若监听局域网地址，必须显式配置并增加认证、来源
限制和防跨站请求保护，不能直接把无认证控制页面暴露给整个家庭网。

## 4. 功能需求

### FR-001：JBL 局域网发现与身份确认

- 支持 `_jbl-product._tcp` 发现；
- 多设备时必须明确选择，不得静默选第一台；
- 使用 IP literal 连接，并通过设备信息确认 Authentics 300；
- 直连局域网，不读取系统 HTTP/HTTPS 代理。

### FR-002：mTLS 与服务端固定

- 客户端证书和私钥在运行时加载；
- 允许在私有研究环境使用两个参考项目提供的凭据；
- 凭据不得进入 Git、程序、包、容器、日志或 Release；
- 请求前必须核对服务端证书 SHA-256 指纹或等价固定；
- 私钥权限过宽时安全失败。

### FR-003：设备与组状态

- 读取并脱敏投影 `getDeviceInfo`；
- 使用 `getAuraCastGroupInfo` 判断保留的成员配置，不把它冒充实时广播/接收状态；
- 只有成员 ID 分别精确匹配私有配置中的 JBL 与 Aura 身份、型号名称正确且组未禁用时，
  才报告“预期设备对配置已验证”；
- 丢弃组 ID、成员 ID、地址、CRC 及未知原始字段；
- 未知声道输出 `unknown`，不直接回显原始值。

### FR-004：Play Together 启动

- 已有双成员配置只能采用为成员身份前置条件；除非当前单写者会话有本轮可信 live
  证据，否则不能仅凭该配置跳过 START；
- 启动前确认目标设备身份和当前状态；
- JBL 与 Aura 命令按照已验证顺序串行执行；
- 写入后重新读取成员配置，确认没有指向错误设备；
- START 还必须获得 JBL 应用层接受与 Aura 精确 AA 回复；设备配置正确但 live 结果未知时
  必须报告未知，不能报告正在双响。
- `JBL_BROADCAST_CONFIRMATION=ack|gena` 是闭集，本机实测固件默认 `ack`：ACK 模式
  返回 `accepted_unconfirmed`、`broadcast_acknowledgement_only` 且 CLI exit `0`；严格
  `gena` 只有匹配动作 `33/34` 才返回 `accepted`、`broadcast_business_notification`；
- managed `linked` 只代表最近一次被控制器接受的动作，不能冒充 `7951` 或声学结果。

### FR-005：Play Together 停止

- 默认按接收端优先的安全顺序解除；
- 正常 `stop` 尽量保留以后无需按键即可再次 `start` 的控制会话；
- 无持久会话或结果未知时，不得假装解除成功；
- `shutdown` 与 `stop` 语义分离。
- 不等待双成员配置消失作为 STOP 后置条件；实机已经证明该配置会在成功 STOP 后保留。

### FR-006：冷启动和恢复

- 通过实时 FDDF 广告、PID 与载荷内嵌稳定身份识别琉璃，不保存轮换 RPA；
- FDDF 广播空窗使用有限次数、有限时长的延迟重试；
- 身份未证明时必须在写命令前停止；
- 单次 LAN 超时不能立即触发蓝牙重建；
- 手机占用、设备关机、网络中断和控制通道断开均应得到不同的安全错误；
- 不允许无限循环或无上限重试。
- 深待机唤醒按动态 HCI 顺序建模：BR/EDR A2DP 自动重连、stored link-key
  authentication/encryption、AVDTP Open，约 `2.5` 秒后 FDDF，App LE 读取更晚；
- production 默认冷链路固定为：stable raw 一次；eligible failure 时仅一次 A2DP
  ConnectProfile（`20` 秒）；`30` 秒内 fresh FDDF 精确身份/PID；DisconnectProfile 并在
  `5` 秒内确认释放；stable raw 重试一次；随后原 LE fallback；全流程共享 `150` 秒 outer
  deadline；
- profile 释放未确认必须在任何角色写前失败。wake module 已进入 neutral artifact 且
  离线全绿；最新无声 no-button cold run 已在 `150` 秒内通过整体链路，但 route 为
  `fresh_le`，A2DP `wake_then_stable` 子路径未单独命中/证明。

### FR-007：服务生命周期

- Ubuntu 支持用户级 systemd 启动；
- 开机默认可建立“就绪”控制条件，但不得自动大音量播放；
- 崩溃或异常终止后，下次启动先识别不确定状态再恢复；
- 同一时刻只能有一个组网写入者；CLI 与 Web UI 必须调用同一个状态机。
- 每次可变设备操作前必须先持久化不含身份信息的 pending 日志；仅在动作被接受且成员
  配置仍精确指向预期设备对后清除；崩溃、timeout 或不确定结果必须跨重启闭锁普通写入；
- `recover-stop` 必须显式确认，并在写前完成安全诊断、稳定身份验真及实时控制身份映射。
- graceful shutdown 的 teardown 错误必须非零传播，并由 teardown latch 防止后续清理掩盖；
- 独立 owner-only `uncertainty.pending` marker 跨重启权威；即使写 clean 时目录 fsync
  失败，普通 mutation 仍闭锁到显式恢复。
- Web accept/serve 错误必须返还 controller actor；正常与异常 listener 退出都只执行一次
  `shutdown_for_exit`。设备安全错误优先；shutdown 成功时仍保留原 `AcceptFailed`，且不
  清除 pending journal。

## 5. 非功能需求

### NFR-001：可靠性

- 成员配置、厂商应用层回复、Aura AA 回复、本地单写者 live 状态和人工声学结果是不同
  证据维度，不得压成一个会误导的线性“组状态”；外部动作或 bearer 丢失后 live 状态
  立即降为 unknown；
- 所有网络、扫描、连接和写入都必须有超时；
- 所有恢复都必须有次数上限和退避；
- 错误不得被吞掉或转换成虚假的成功状态。
- v0.4 只能由操作者明确选择；Rust 在写前拒绝或结果不确定后不得自动切换后端、重放
  命令或把两种后端的证据拼成一次成功。
- 防火墙规则只能作为主机诊断条件。即使授权安装窄规则后 strict GENA 仍 timeout，也
  不得把“规则已安装”写成协议成功或 `7951` 证据。

### NFR-002：资源

- Ubuntu 目标空闲内存低于 50 MiB；
- Ubuntu 目标空闲平均 CPU 低于 1%；
- 不为低频 I/O 控制引入大模型或常驻浏览器进程。

### NFR-003：安全与隐私

- 所有公开文件只使用合成占位符；
- 检查普通形式和十六进制编码的 MAC/IP；
- 检查当前树和 Git 历史；
- API 响应限制为有界大小；
- 日志默认脱敏；
- Web UI 默认仅本机访问；
- 发布前人工审查 staged diff 和产物内容。

### NFR-004：发布与可维护性

- Rust 开发工具链固定为 1.96.0，不跟随 moving stable；
- Cargo 使用锁文件和 `--locked`；
- 只在安全修复、必要依赖或 Windows 阶段有明确原因时升级；
- Rust 源码通过 `rustfmt`、Clippy、单元测试和 release 构建；
- 完成 Ubuntu 目标所需的包允许安装，但必须最小化、隔离并记录用途；
- Python 参考路线保留到 Rust 实机功能等价；
- 不直接复制无明确许可证的上游源码。

## 6. 证据等级

以下结果必须分别记录，不能互相冒充：

1. 传输写入 ACK；
2. 厂商应用层成功回复；
3. JBL 设备报告的预期双成员配置（不是实时 linked 证明）；
4. 人工确认两台音响都出声；
5. 标准 BASS/BASE/BIG/BIS/ISO 数据面证明。

当前项目已经证明第 3 级的成员配置和第 4 级的部分 v0.4 真实场景，但没有证明第 5 级。
2026-08-30 约 `03:45` 的 Rust 完整歌曲尝试与 Home Centre 自动 STOP 重叠，不能用于
协议结论。后来 clean Rust 默认事务不含 `7957`：Aura AA ON 与 JBL Wi-Fi ENTER 均
accepted，本地状态成为 `linked`；EOF 修复后等待 `15` 秒再仅向 JBL 网络播放，用户
仍确认 JBL 响、琉璃不响。这否定目标方向的无 `7957` 设计并排除原 `2` 秒过短是唯一
原因，固定 `10.5`/`15` 秒不是修复。simultaneous official run 的音频为 Android
A2DP→Aura，Aura PRIMARY/JBL RECEIVER；目标 JBL-source 重新纳入独立 Assistant
`7957` broadcaster 与 Aura AA receiver 的跨状态机组合。exact GATT `0x002a` 首轮
START accepted，MA 仅向 JBL 网络以请求的 `5%` 播放，用户确认两台都响；这是首轮 Rust
目标方向声学通过。普通 STOP 因 aura_ack_timeout outcome-unknown，recover-stop 在
`13` 秒内 accepted/ready。fresh-bearer 修复后第二轮再次双响，音乐 idle 后普通 STOP
约 `43` 秒 accepted/ready、无需 recovery；按约定停止声音测试。没有 `7951` 或第 5 级
证据，也不宣称该组合是一条官方 UI 序列；整体 no-button cold 已通过，但 A2DP
`wake_then_stable` 子路径、P0 与发布仍未完成。

窄 UFW 规则获授权并由用户安装后，production strict GENA 静默 START 仍以
`jbl_broadcast_result_timed_out` 失败，随后由 legacy GATT 归一化；因此当前 exact
firmware 未实证 `7951`，防火墙规则也不是协议成功。最新 HCI 进一步证明深待机首先由
Android 系统 BR/EDR A2DP 自动重连，经 stored link-key 鉴权/加密与 AVDTP Open，约
`2.5` 秒后才出现 FDDF，App LE 读取更晚。wake module 已 production 接入并进入最新
neutral artifact；stable raw → 单次有界 A2DP wake → fresh FDDF exact gate → 确认释放
→ stable raw retry → LE fallback 全程共享 `150` 秒 deadline。释放未确认即写前失败。
当前 `258` lib + `8` CLI（主 harness `266`）、FIFO private-file helper `1/1`，以及
audit/deny/fallback/privacy/neutral 门禁通过，compat evidence mode 完成。最新无声无按键冷启动在无活动音频流、journal clean、没有已解析的 BlueZ
现成 session 条件下，START `122.15` 秒 accepted_unconfirmed/linked；status 双成员
verified/healthy、Aura route `fresh_le`；STOP `15.89` 秒
accepted_unconfirmed/ready，最终 journal clean、`NRestarts=0`。手机 App 未参与本轮，
但无 ADB 手机状态证据。整体 no-button cold path 已实机通过；A2DP
`wake_then_stable` 子路径本轮未命中/证明。

最终 neutral artifact 为 `8,284,440` bytes、`GLIBC_2.34`，动态依赖仅 `libc`/
`libgcc`；artifact/installed/process hash 一致，具体 SHA 不进入公开正文。user systemd
重启后 enabled+active、`NRestarts=0`；一次只读 status 后 managed unknown/offline，
`15` 秒 restart-idle 采样为 RSS `8,828 KiB`、`1` thread、`15` fds、平均 CPU
`0.0667%`（`1` tick）。loopback Web/status 已验证两个脱敏成员、allowlisted
channel、`last_action`/`age_ms`，CSP/CSRF 保持。

## 7. Ubuntu 验收门槛

Ubuntu v0.5 稳定版至少满足：

1. 全新私有配置可以完成 `doctor`；
2. Rust LAN 客户端读取、私有成员身份与配置验证通过；
3. 已有 App 建组可以无破坏采用；
4. 至少两轮无按键冷启动通过；
5. 至少两轮连续 `start -> stop` 通过；人工双响完成两轮后不再为刷次数播放，后续周期可
   在无音乐条件下补齐控制生命周期证据；
6. 测试一次 FDDF 空窗安全失败与稍后恢复；
7. 测试手机占用、网络中断、音响关机和进程重启；
8. Web UI 与 CLI 显示同一状态，不发生双写者竞争；
9. 人工完成双响确认；
10. 测量 CPU、内存、启动时间和可执行文件体积；
11. 完整离线测试、Clippy、隐私扫描、Git 历史扫描通过；
12. 公开产物不含任何证书、私钥、真实设备/家庭网络标识。

两轮冷启动成功后不为“刷次数”继续破坏性重复测试；后续只补尚未覆盖的失败场景。

## 8. 明确不在当前范围内

- QQ 音乐、微信登录或其他音乐账号；
- Music Assistant 本体；
- 小米电视或三设备同步；
- 独立 AirPlay/A2DP 双路延迟校准；
- 通用 JBL 全型号支持；
- 公开 APK、固件、抓包、反编译源码或厂商凭据；
- 在尚未抓到数据面证据时宣称标准 BASS/BIG/BIS 已证明；
- Ubuntu 完成前并行开发 Windows 专用实现。

音量、静音、EQ、播放源与 Home Assistant 可作为后续独立需求评审，不自动进入当前
Play Together 稳定版范围。

## 9. 当前实现状态

截至 2026-08-30：

- v0.4 Python/BlueZ 路线保留且具有真实硬件成功记录；
- Rust 1.96.0 已固定；
- Rust alpha 已在 Ubuntu 编译为单一 ELF；
- Rust 单元测试、格式与 Clippy 已通过；
- Rust 已只读实机取得 Authentics 300 脱敏信息，并验证保留的双成员配置；实机 STOP
  对照已证明该配置不能单独代表实时关联；
- Rust 已实现 `start`、`stop`、`serve`、用户级 systemd、原生 FDDF/GATT 与整对状态机；
- Rust 原生 `start/stop` 已被真实设备接受；两轮无按键冷 `start` 第一轮 managed 状态
  曾报 `br_edr`、第二轮最终 `le`，持久会话正常 `stop` 约为 0.44 与 0.57 秒；
- 一次真实 FDDF 空窗在写前安全拒绝并留下 clean；直接连接 LE `Device1` 失败后，已配对
  trusted 稳定 public 对象触发 BlueZ 连接，vendor GATT 位于唯一 connected random
  对象；该对象经精确 FDDF PID/稳定身份验真后，显式恢复接受并回到 `ready`；
- 写前 journal 已实证在旧 timeout panic 后跨崩溃保留 pending；timeout 已修，普通写入
  在显式恢复前保持闭锁；
- Rust 日常服务使用 `8096`，与 Music Assistant `8095` 分离；v0.4 保留但无自动 failover；
- v0.4 与 Rust 共享 owner-only operation/session 锁；启用其中一个服务前必须明确停用
  另一个，不能并发写设备；
- `03:45` Rust 听感尝试受自动 STOP 污染，不能作为失败；后来 clean Rust 默认事务
  不含 `7957`，EOF-fixed AA ON 与 Wi-Fi ENTER accepted/local linked 后等待 `15` 秒
  仍仅 JBL 出声；exact GATT `7957` 首轮 START 双响通过，但普通 STOP
  aura_ack_timeout/outcome-unknown，仅显式 recovery 回 ready；fresh-bearer 修复后第二轮
  双响且普通 STOP 约 `43` 秒回 ready；无声 no-button cold 经 `fresh_le` 在 `150` 秒内
  通过，`7951`、A2DP `wake_then_stable` 子路径与其余发布矩阵未完成；
- 最终冻结已记录 `258` lib + `8` CLI（主 harness `266`）、FIFO private-file helper `1/1`，
  neutral ELF `8,284,440` bytes、`GLIBC_2.34`、
  `libc`/`libgcc`，安装 hash 一致，以及 `15` 秒 idle RSS/线程/fd/CPU 样本；具体 SHA
  仅保留在 release 内部证据；
- Windows 实现尚未开始，符合既定顺序。
