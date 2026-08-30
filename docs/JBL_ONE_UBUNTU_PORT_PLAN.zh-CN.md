# JBL One 控制逻辑 Ubuntu clean-room 迁移计划

- 状态：执行中
- 日期：2026-08-31
- 固定 App：JBL One 2.7.9
- 首个实机：JBL Authentics 300
- 实施平台：Ubuntu 22.04 / Rust 1.96.0

## 1. 新目标

继续拆解 JBL One 的发现、连接、OneOS HTTPS、UPnP、可选 WebSocket、媒体、音量、静音、
播放、音源、EQ 和产品设置逻辑，把证据闭合的行为尽可能迁入当前 Ubuntu Rust 单可执行
程序。

迁移遵循：

```text
私有 App/实机证据
  -> 独立表述的协议事实
  -> 完全合成的公开 fixture
  -> 原创 Rust 类型与实现
  -> 离线负例/边界测试
  -> JBL 单项实机读写验收
```

禁止把 APK、JADX/Smali 源码、抓包、证书、密钥、真实地址或原始设备响应复制到公开
仓库。公开程序不提供 raw HTTP、raw command、任意 GATT write 或任意 JSON payload。

## 2. 实机授权与安全边界

操作者允许持续测试 JBL Authentics 300。任何可能出声的测试必须先读回并把设备音量保持
在 `0–9%`。

允许：

- 只读设备、功能、媒体、音源、EQ 和产品设置；
- 音量、静音、播放控制、音源、EQ 和可恢复设置的单项有界写入；
- 每项写前快照、一次动作、写后读回、必要恢复；
- HTTPS/UPnP 和只有运行时 capability 明确允许时的 WebSocket。

禁止：

- OTA、固件下载触发、刷写、恢复出厂、关机或 reboot；
- 设备身份、Wi-Fi、账号、证书、语音助手和不可逆许可证写入；
- 写后 timeout 自动改走另一 bearer 或自动重发；
- 任意命令枚举和无上限重试；
- 本计划内操作 Aura Studio 5 或改变 Play Together 组。

任一写入出现 timeout、断连、响应格式变化、读回冲突或无法恢复时，立即停止该功能，记录
`outcome_unknown`，不继续下一种 payload。

## 3. 已闭合的网络选择

JBL One 2.7.9 的精确选择规则：

```text
Wi-Fi online + feature_support.websocket_connect.support
  -> ws://IP:8080

否则 Wi-Fi online
  -> pinned mTLS HTTPS /httpapi.asp + UPnP

否则
  -> Bluetooth session
```

不存在一次命令失败后跨 bearer 重放。

本机 2026-08-30 只读复核：

- `getFeatureSupport` 成功；
- `feature_support` 没有 `websocket_connect`；
- TCP 8080 关闭/拒绝；
- 因此当前 Authentics 300 路由固定为 mTLS HTTPS + UPnP，WebSocket 仅保留静态候选，
  不实现为当前设备的生产主路。

## 4. 阶段与门槛

### P0：基线与防回归

- 公开工作树基线、固定 Rust 1.96.0；
- 原 Play Together 测试、privacy、history scan 全绿；
- 三项旧控制服务不并发占用设备；
- JBL 音量读回 `<=9` 后才允许声音测试。

### P1：类型与状态基础

- exact Authentics 300 -> OneOS 路由；
- exact Aura tuple 与本计划隔离；
- Official Home 与 JBL-source compatibility flow 分离；
- command/scanner/retained/acoustic 四维 reducer；
- capability maturity：implemented read-only / research-only / serializer-only /
  evidence-required / forbidden。

### P2：OneOS HTTPS 与 UPnP 只读面

- 封闭 `OneOsReadCommand`；
- `getDeviceInfo`、`getFeatureSupport`；
- UPnP `GetInfoEx`；
- `getMediaSource`、`getMediaSourceStatus`；
- `getDeviceAudioSourceList`；
- `getEQList/getEQ`；
- `getProdSetting`、Personal Listening、Audio Sync；
- 未知字段只投影为 `unknown`，不回显原文。

实机通过条件：型号经 pinned mTLS + UPnP 双重绑定，响应大小有界，解析 DTO 无身份信息，
连续两次只读结果结构一致。

### P3：音量

- UPnP `GetInfoEx` 读取百分比和 mute；
- UPnP `SetVolume` 使用 `Channel=Single`；
- WebSocket 0–32 档映射只保留 research fixture；当前设备不启用；
- CLI 写入必须显式确认并经过单写者锁；
- 默认拒绝高于安全上限的值。

实机顺序：快照 -> 设 `9` -> 读回 `9` -> 设另一 `0–9` 值 -> 读回 -> 恢复 `9`。

### P4：静音

- 读取 `CurrentMute`；
- 写入仅使用已闭合的 UPnP `SetMute`；
- 不猜 WebSocket `mute_status` 写法，因为官方 App 没有对应调用链；
- on/off 各一次并读回，最后恢复写前状态。

### P5：播放控制

- 按来源能力区分 Play/Pause/Stop/Next/Previous；
- 当前蓝牙来源先只验证 Play/Pause，不假设 next/previous 可用；
- 所有可能出声的动作保持 `<=9%`；
- 写后以 `GetInfoEx` 状态变化为后置条件。

### P6：音源

- 先读取 `getDeviceAudioSourceList`；
- 仅允许列表实际返回且可恢复的 source；
- 一次切换、读回、恢复原 source；
- 不把 soundbar HDMI/TV token 强加给 Authentics 300。

### P7：EQ

- 严格解析 5 个 preset 和当前 `eq_setting`；
- 保留设备返回的实际结构，不强套旧 bass/treble 模型；
- 首次只做预设切换：快照 -> 另一已返回 preset -> 读回 -> 恢复；
- 频段写入需另一个独立门槛，不能与预设切换合并。

### P8：产品设置

- 固定 key：PureVoice、FlexListening、DeepSleep、SmartDetails；
- 先逐 key 只读并建立类型；
- 只有存在明确当前值、合法枚举和恢复值才允许写；
- 不触碰账号、OTA、工厂、网络或语音助手设置。

### P9：CLI、Web 与发布门槛

- CLI：`media`、`capabilities`、后续 `volume/mute/source/eq`；
- Web 先增加只读卡片，写按钮逐项随实机验收启用；
- CLI/Web 共用单 actor 和 revision；
- 全量 fmt、Clippy、all-target tests、Python/Bash fallback、privacy、Git history、
  neutral release scan；
- 文档按“协议已迁移 / 实机只读 / 实机写入 / UI 已启用”四级记录。

## 5. 当前进度

- P1 纯类型模块已实现并通过离线测试；
- P2 的 `media`、`inspect`、`capabilities` Rust CLI 已实现；`inspect` 固定读取 7 个
  typed 接口且不输出名称、ID、未知 token 或 EQ 原始值；
- JBL 实机只读 `media` 与 `inspect` 已通过；
- P3 Rust 单写入实机顺序 `9 -> 8 -> 独立读回 8 -> 9 -> 独立读回 9` 已通过，
  全程保持暂停；
- 音量生产入口固定拒绝 `>9%`，要求不可伪造的单写者锁；写前已是目标时零写入，
  写回包丢失仅记弱证据而不返回成功；
- UPnP 写后增加 pinned-mTLS 身份复核，负例覆盖 pin/model/缺失旧值、写后断连、
  不重试、读回冲突与 CLI 边界；
- 端口 59152 的 UPnP 本身是明文且未认证；前后 pinned-mTLS、固定 IP 与 exact model
  只能降低误投，不能加密或密码学绑定中间的 UPnP 写入/读回。局域网仍属于信任边界；
- P4 实机发现 A300 `GetMute/SetMute` 必须使用 `Channel=Master`；`Single` 返回 fault
  402。Rust `mute-set on/off` 已分别写入并独立读回，最终恢复未静音；
- 只读写前网络失败允许最多 3 次、100 ms 间隔的同 bearer 有界重试；身份错误、HTTP/
  解析错误、任何写入及写后读回一律不重试；
- WebSocket capability 未广告且 8080 关闭，当前不走 WS；
- EQ/音源/Personal Listening/Audio Sync 的 schema-only 实机读取已完成；
- 两个固定开源上游仅作行为参考：当前直接复制为零；原创 Avahi/D-Bus
  `_jbl-product._tcp.local.` 候选发现已完成真实脱敏扫描，仍不自动选择设备。一次 3 秒
  scan 在 Avahi 仍可 resolve 时漏过初始事件，因此 CLI 改为单次固定 5 秒窗口、不重试；
- P5 已闭合官方 App exact 路径：当前无 WebSocket 时确实使用 discovered UPnP
  AVTransport Play/Pause，且没有 HTTPS/BT fallback。首次 Bluetooth `Play` 在预先确认
  静音、9% 条件下由设备返回 HTTP 500 / SOAP fault 501 `Action Failed`，状态为
  `Stopped`；没有重试或盲发 Pause，最终解除静音。当前无活跃 Bluetooth 媒体会话时
  Play 明确不可用，生产成熟度保持 evidence-required；
- Ubuntu 已配对/信任 JBL，普通 BlueZ connect 可建立控制/GATT bearer，设备也广告 Audio
  Sink；但显式 A2DP ConnectProfile 超时，完整 disconnect/connect 后仍只有原有一张蓝牙
  音频卡，没有形成 JBL A2DP 媒体会话。实验后 JBL 已恢复未连接；
- P6 实机动态 source 列表为 `AUX / USB / BT`。官方 exact raw HTTPS 请求要求保留
  `/httpapi.asp?` 空 query；因 `ureq` 会移除它，Rust 使用同一 pinned TLS connector 上的
  封闭 HTTP/1.1 writer。AUX 与 BT 恢复均已写入并在固定 350 ms 后一次读回确认；切换
  source 会清除 mute，因此安全边界始终以 `volume<=9` 为准；
- P7 exact feature/catalog gate 为 7-band preset EQ。Rust 只开放 Signature、Vocal、
  Energetic、Chill，完整复用设备返回的 ID/fs/gain 且不公开这些值；CUSTOMIZE(id=0)
  因需要调用方 gain 明确禁止。Vocal 与恢复 Signature 均已写入、350 ms 读回，并各用
  `already_at_target` 零写分支再次确认；
- P8 exact `getProdSetting` 的无参数 GET 与四个固定 key POST 均只返回
  `error_code`，没有返回设置 map；当前固件不能按 App 静态 DTO 假定可读，保持
  evidence-required，不开放写入；
- 音源与四个非自定义 EQ 预设已经开放；播放与产品设置写入仍关闭。

## 6. 完成定义

本轮目标完成需要：

1. 高价值读取全部进入 Rust typed DTO 和 CLI/Web 只读面；
2. 音量、静音、播放、音源和 EQ 至少按上述门槛逐项判定为 verified 或明确 blocked；
3. 不存在 raw-command 逃生口、跨 bearer 自动重放或未经验证的成功状态；
4. JBL 最终音量 `<=9%`、无遗留播放；
5. 公开仓库所有离线、隐私和发布门槛通过；
6. 私有证据索引记录官方 App 定位与每次实机前后状态，公开仓库只保留合成事实。
