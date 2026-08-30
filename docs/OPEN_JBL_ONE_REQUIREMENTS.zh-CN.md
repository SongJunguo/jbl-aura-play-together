# Open JBL One 开源替代产品需求

- 工作名称：Open JBL One
- 状态：产品目标已确认，主仓库待建立
- 日期：2026-08-30
- 首个实机基准：JBL Authentics 300 + Harman Kardon Aura Studio 5
- 平台顺序：先 Ubuntu 22.04，完成后迁移到 Windows 11

## 1. 总目标

构建一个本地优先、开源、低资源的 JBL One 替代控制器，尽可能覆盖以下两个公开项目
已经提供或记录的功能，并加入它们都没有完成的 Play Together 控制：

- <https://github.com/k1rnt/jbl-soundbar-cli>
- <https://github.com/MrBearPresident/JBL_Soundbar>

产品不应只是一组研究脚本。最终用户在每个平台获得一个主程序，通过 CLI 或内置 Web
UI 完成设备发现、状态查看和安全控制。首个完整支持型号是 Authentics 300；其他 JBL
One/OneOS 型号必须通过能力探测逐步加入，不能仅凭“同系列”宣称兼容。

## 1.1 最高优先级：Play Together

Play Together 是本项目区别于两个参考项目的独有核心能力，也是当前唯一 P0 主线。

在以下目标全部完成前，音量、EQ、通用播放、源切换、模式、按键和 Home Assistant 不
进入生产写入或发布承诺。用户已授权把官方 App 已闭合的部分提前迁为 P1 离线 Rust
协议模型、只读状态、合成 fixture 和默认未启用的 mutation serializer；它们不得操作
音响或冒充已经验收的功能：

1. 无手机 App 的正常 `start/stop/status`；
2. 设备报告的 JBL + Aura 双成员配置与私有身份验真，并与实时状态分离；
3. 无反复按蓝牙键的冷启动；
4. FDDF 广播空窗、手机占用、断网、关机与进程重启的有界恢复；
5. 开机服务和单写者状态机；
6. 只围绕组网核心状态与操作的最小 CLI/Web UI；
7. 两轮冷启动、两轮连续 start/stop 和人工双响验收；双响完成两轮后，后续周期只做
   无音乐控制验收。

“开源 JBL One 替代”是长期产品方向，不得被理解为当前应该同时铺开所有普通控制。

## 2. 仓库边界

建议采用两个仓库：

1. 新的通用主仓库（工作名 `open-jbl-one`）：Rust 核心、CLI、Web UI、通用 JBL LAN
   控制、配置、服务、Home Assistant 接口和发布产物；
2. 当前 `jbl-aura-play-together`：保留 v0.4 历史、协议证据、冷发现与这对设备独有的
   Play Together 蓝牙后端，后续由主程序调用或迁入明确许可的原创模块。

这样不会把已经公开且证据边界清楚的“固定设备对实验仓库”硬改成通用产品，也便于将来
支持 Authentics 200/500 或其他 OneOS 设备。

在新主仓库建立前，本文件与 `FEATURE_PARITY.zh-CN.md` 是迁移种子文档，不表示当前
v0.4 已经拥有所有产品功能。

## 3. 代码与上游使用规则

用户授权在私有研究环境中直接使用、运行、修改和比较两个参考项目的代码、证书、私钥
和设备标识。这些材料不得因此自动进入公开仓库。

公开代码复用遵循：

1. 有明确、兼容许可证：允许复制，记录仓库、commit、文件、作者与许可证，在
   `NOTICE` 中保留归属；
2. 无许可证但取得作者书面授权：按授权范围复用并保存授权记录；
3. 无许可证且无授权：只吸收功能需求、接口事实、协议行为和通用架构，代码独立实现；
4. 凭据、私钥、证书、真实设备标识永远只从仓库外私有运行目录加载；
5. 禁止把参考项目的嵌入私钥方案复制到公开源码或可执行文件。

截至 2026-08-30，GitHub API 对两个主仓库均未识别到仓库级许可证，`/license` 均返回
404；Rust 项目只在 `Cargo.toml` 声明 MIT。因此当前公开主线默认采用独立实现，除非
后续补齐许可证或作者授权。

## 4. 用户交付形式

每个平台目标为一个主可执行文件：

```text
Ubuntu: open-jbl-one
Windows: open-jbl-one.exe
```

同一程序提供：

```text
doctor
discover
status
play / pause / stop / next / previous
volume / mute
source
eq
mode
button
group start / group stop / group status
serve
install-service
```

`serve` 启动编译进程序的本地 Web UI。源码保持模块化，发布结果是单文件。配置、证书、
私钥和设备标识仍保存于外部私有目录。

## 5. 功能需求

### F-01 设备发现与身份

- 通过 `_jbl-product._tcp` 发现 OneOS/JBL 设备；
- 支持手动 IP，但必须验证设备信息；
- 多设备不得默认选第一台；
- 记录稳定、脱敏的设备配置，不在公开输出暴露 UUID/MAC；
- Authentics 300 是首个必须完整实测的型号。

### F-02 LAN/mTLS 安全客户端

- 使用运行时客户端身份访问 `https://<IP>/httpapi.asp`；
- 禁用系统代理，限制超时、重定向和响应大小；
- 固定设备服务端证书或指纹；
- 兼容实测 X.509 v1 证书；
- 原始 JSON 只在私有诊断模式使用，正常层只输出 allowlist 字段。

### F-03 设备信息与在线状态

- 型号、名称、固件、OneOS 版本；
- 网络与 API 可用性；
- 当前输入源；
- 播放状态、音量、静音；
- 能力列表和不支持原因；
- 不用固件字符串的脆弱启发式代替真实能力探测。

### F-04 播放控制

- Play、Pause、Stop、Next、Previous；
- 区分蓝牙、AirPlay、DLNA/网络流等输入源的控制能力；
- HTTP 成功不等于动作成功，尽可能写后读回；
- 不支持的源显示为“不支持”，不能静默失败。

### F-05 音量与静音

- 读取使用已验证的 UPnP `GetInfoEx`；
- 音量写入优先 UPnP `SetVolume`、`Channel=Single`；
- 0–100 严格校验，超过安全默认上限要求显式确认；
- 静音提供幂等 on/off，不把 toggle 冒充幂等设置；
- 所有写入后读回验证。

### F-06 输入源

- 读取 `getMediaSource`；
- 只显示设备真实支持的源；
- 蓝牙、网络等源的切换 token 逐型号实测；
- 不把 soundbar 的 HDMI/TV/Atmos/rear 实体错误暴露给 Authentics 300。

### F-07 EQ

- 读取 EQ 列表、当前预设和频段；
- Authentics 300 首个基准为 5 个预设、7 段数据；
- 解析实际 `eq_list/eq_setting`，不能强套旧 `bass/treble/bands` 结构；
- 预设切换和单频段写入需要范围检查、原值快照、写后读回和恢复路径；
- 未知结构安全拒绝，不丢弃后回写未知字段。

### F-08 模式与设备设置

能力探测后逐项实现：

- Personal Listening Mode；
- PureVoice；
- Surround/Night/Display 等参考项目公开的模式；
- Audio Sync；
- 其他 Authentics/OneOS 返回的安全本地设置。

每项必须区分：只读已验证、写入已验证、其他型号候选。接口名称相近不能视作兼容。

### F-09 模拟按键

- 建立 Authentics 300 实测 token 表；
- 支持音量、播放暂停、蓝牙/源等已验证动作；
- 禁止默认开放任意 token/任意 payload；
- power 等 toggle 动作必须明确标记非幂等，不能提供虚假的 on/off 状态保证。

### F-10 Play Together

- 读取并脱敏 `getAuraCastGroupInfo`；
- 建立、解除、采用与恢复 JBL Authentics 300 + Aura Studio 5 组；
- 日常操作不要求手机 App 或反复按键；
- 使用设备报告双成员配置排除错误设备，但不把持久配置冒充实时 linked；实时 managed
  状态、控制回复与人工双响分别验收；
- 传输 ACK、应用回复、拓扑验证、人工双响和 BASS/ISO 证据严格分级；
- 保留已经验证的 v0.4 Ubuntu 后端，直到 Rust 等价替代通过。

### F-11 Web UI

- 一个二进制内嵌 UI；
- 默认只监听 `127.0.0.1`；
- 首页显示设备、播放、音量、静音、源、EQ、模式和 Play Together；
- 普通控制与危险恢复分区；
- CLI、Web UI、服务共享单一写入者和同一状态机；
- 局域网开放时必须显式启用认证与 CSRF/Origin 防护。

### F-12 Home Assistant

- 通用协议库与 UI 解耦；
- 提供标准 `media_player` 行为；
- 按能力提供 select/number/switch/button/binary_sensor；
- 使用共享异步连接、可用性和退避，不串行新建大量会话；
- Play Together 写入由单写者服务执行，HA 不复制状态机。

### F-13 服务、日志与升级

- Ubuntu 用户级 systemd；Windows 后续提供服务/任务计划；
- 日志脱敏、有界轮转；
- 崩溃恢复与最后动作状态；
- 设备固件版本与项目兼容性提示；
- 初期只读取固件信息，不自动刷写固件。

## 6. 非功能需求

- Rust 源码模块化，发布为单可执行文件；
- Ubuntu 阶段固定 Rust 1.96.0 与 Cargo.lock；
- 空闲内存目标低于 50 MiB、平均 CPU 目标低于 1%；
- 读操作默认安全，写操作串行、有限重试、写后验证；
- 隐私扫描覆盖当前树、Git 历史、普通与十六进制标识；
- 所有功能有合成 fixture、错误路径测试和型号能力矩阵；
- Windows 只在 Ubuntu 稳定后开始，不并行分散主线。

## 7. 里程碑

### M0：安全 Rust LAN 核心

- 已完成：运行时凭据、指纹固定、设备信息、双成员配置、配置解析、Ubuntu 单文件；该
  配置已证明不能冒充实时 linked 状态。

### M1：Rust Play Together 主线

- 已完成 checkpoint：Rust 双成员私有身份/配置验真已接入
  `start/stop/status/recovery`，并与 managed live 状态分离；
- 已完成 checkpoint：原生 Aura FDDF 冷发现、稳定 public 对象触发连接后对 connected
  random GATT 对象的精确身份门，以及持久控制会话已获真实设备接受；
- 保留 v0.4 Python/BlueZ 后端，直到 Rust 完成等价验收；
- 两轮无按键冷启动已完成；约 `03:45` 的旧尝试受 Home Centre 自动 STOP 污染，不能
  用于协议结论；EOF-fixed clean Home-flow-only 无 `7957` 事务 accepted/local linked，
  等待 `15` 秒再向 JBL 网络播放仍仅 JBL 出声。官方 Home flow 实证为 Android
  A2DP→Aura、Aura PRIMARY/JBL RECEIVER；目标 JBL-source 因而重新纳入独立 Assistant
  `7957` broadcaster 与 Aura AA receiver 的跨状态机组合。exact GATT `0x002a` 首轮
  START accepted，MA 仅向 JBL 网络以请求的 `5%` 播放时用户确认两台都响；普通 STOP
  aura_ack_timeout/outcome-unknown，recover-stop 在 `13` 秒内回 ready。fresh-bearer 修复
  后第二轮再次双响，音乐 idle 后普通 STOP 约 `43` 秒 accepted/ready、无需 recovery。
  按约定停止声音测试；production no-button cold 已通过一轮 `fresh_le` 实机验收，A2DP
  `wake_then_stable` 子路径未命中；`7951`、P0 与发布仍未完成。

### M2：Play Together 最小 Web UI 与服务

- 已实现：`serve` 提供组状态、start、stop、刷新；明确恢复保留在 CLI 隐藏确认入口；
- 已实现单写者、systemd 与错误状态；最终资源测量和发布冻结仍待完成。

### M3：覆盖两个参考项目的通用控制

- 播放状态/控制、音量、静音、源、EQ、模式、按键；
- 每项在 Authentics 300 上完成读/写/读回证据；
- 不得反向破坏 M1/M2 的 Play Together 可靠性。

M1 尚未冻结时，M3 可提前完成纯离线协议移植与只读 surface，但不得开放未经实机验收
的写按钮、自动 fallback 或任意 raw command。

### M4：Ubuntu 稳定发布

- 安装文档、单文件产物、隐私/许可证审计、Home Assistant 接口。

### M5：Windows 11

- 仓库迁移后实现 WinRT/BLE、ACL、服务和 `.exe`，重跑同一验收。

## 8. 当前非目标或延后项

- 厂商云账号、QQ 音乐/微信登录等音乐服务凭据；
- 未经验证的 Wi-Fi 首次配网替代；
- 自动固件下载或刷写；
- 在没有数据面抓包时声称标准 BASS/BIG/BIS 已证明；
- 对所有 JBL 型号做未经实测的统一承诺。

这些功能可以后续立项，但不能因“JBL One 替代”四个字自动视作已经授权或已经完成。
