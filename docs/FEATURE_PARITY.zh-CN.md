# Open JBL One 功能覆盖矩阵

当前实机/发布优先级：`Play Together start/stop`、冷 FDDF 自动发现、双成员状态、服务
与最小 Web UI 为 P0。普通 JBL 控制的生产写入仍为 P2；官方 App 已闭合的网络类型、
只读状态和合成 fixture 可作为 P1 离线迁移，不因此把矩阵中的“300 写”或“UI”改成是。

状态含义：

- `参考`：上游公开项目存在该功能或协议线索；
- `300 读`：已在 Authentics 300 读取验证；
- `300 写`：已在 Authentics 300 安全写入并读回或确认后果；
- `Rust`：当前 Rust 主线已经实现；
- `UI`：内置 Web UI 已接入；
- `待办`：尚未完成或仍需逐源/逐型号验证。

| 功能 | 参考 | 300 读 | 300 写 | Rust | UI | 当前结论 |
|---|---:|---:|---:|---:|---:|---|
| mDNS `_jbl-product._tcp` | 是 | 是 | 不适用 | 部分 | 否 | Avahi/D-Bus adapter与真实候选扫描已完成；exact身份绑定/显式选择待办 |
| mTLS `httpapi.asp` | 是 | 是 | 是 | 是 | 否 | Rust 使用运行时身份与服务端指纹固定 |
| 设备信息/固件/OneOS | 是 | 是 | 不适用 | 是 | 否 | Rust 已实机脱敏读取 |
| Play Together 成员配置 | 否 | 是 | 不适用 | 是 | 否 | Rust 已实机验证 JBL + Aura 两成员；STOP 对照证明它不是 live 状态 |
| UPnP 全局播放状态 | 是 | 是 | 不适用 | 是 | 否 | `GetInfoEx` strict SOAP parser 已实机通过 |
| `getPlayerStatus` | 是 | 是 | 不适用 | 否 | 否 | 对外部蓝牙曾误报 stop，仅作次级 metadata |
| 音量读取 | 是 | 是 | 不适用 | 是 | 否 | `GetInfoEx` 已验证 |
| 音量写入 | 是 | 是 | 是 | 是 | 否 | `SetVolume/Single`、0–9 上限、单写者与独立读回已验证 |
| 静音读取 | 是 | 是 | 不适用 | 是 | 否 | `CurrentMute` 已验证 |
| 静音写入 | 是 | 是 | 是 | 是 | 否 | A300 必须 `SetMute/Master`；on/off 均已写入、读回并恢复 |
| Play/Pause/Stop/Next/Previous | 是 | 部分 | 未通过 | 部分 | 否 | 官方无WS时确走UPnP；当前BT/Stopped返回SOAP fault 501且无fallback，生产仍关闭 |
| 当前输入源 | 是 | 是 | 不适用 | 是 | 否 | `getMediaSource` 已验证 |
| 输入源切换 | 是 | 是 | 是 | 是 | 否 | exact AUX/USB/BT动态列表；AUX→BT均写入并在350ms单次读回确认 |
| EQ 列表/当前值 | 是 | 是 | 不适用 | 是 | 否 | 脱敏 typed inspection：5 预设、当前 3 个 payload 数组项 |
| EQ 预设/频段写入 | 是 | 是 | 预设是 | 预设是 | 否 | Vocal→Signature均写入/读回/零写确认；CUSTOMIZE与自定义频段仍禁止 |
| Personal Listening Mode | 是 | 是 | 待办 | 否 | 否 | 读接口已验证 |
| PureVoice/ProdSetting | 是 | 未通过 | 否 | 否 | 否 | exact GET/四key POST均仅回error_code、无设置map；禁止猜值或写入 |
| Surround/Night/Display | 是 | 未确认 | 未确认 | 否 | 否 | 必须能力探测，不暴露 soundbar 假实体 |
| Audio Sync | 是 | 是 | 待办 | 否 | 否 | 读接口已验证 |
| 模拟按键 | 是 | 候选 | 待办 | 否 | 否 | 两上游 token 不同，不能盲目合并 |
| Play Together start/stop | 否 | 是 | 是 | 是 | 是 | 两轮 START 双响、post-fix STOP、no-button cold `fresh_le` 通过；A2DP wake_then_stable/`7951`/P0 发布未完成 |
| 冷 FDDF 自动发现 | 否 | 是 | 是 | 是 | 是 | 稳定 public 对象只触发连接；connected random 对象仍须精确 FDDF 验真 |
| Home Assistant | 是 | 不适用 | 不适用 | 待办 | 待办 | 独立重写标准 media_player/coordinator |
| 内置 Web UI | 否 | 不适用 | 不适用 | 是 | 是 | 单二进制、本机 `8096`、共享单写者；当前代码门禁已过，最终产物冻结待修订事务完成 |

该表必须随每次真实设备实验更新。HTTP 200、传输 ACK、设备拓扑和人工听见属于不同证据，
不得为了把表格填成“是”而降低验收标准。
