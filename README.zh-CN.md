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

最终的 v0.3 自动管理器又在不再次按键的情况下完成了 4 次 START、3 次 STOP；其间
PulseAudio 蓝牙模块恢复，用户级 systemd 服务跨命令保持存活。一次紧接着执行的
`shutdown / start` 冷重建也曾成功，但后来等琉璃的可连接窗口关闭后再次冷重建，尚未
发送命令便返回 `Host is down`。因此准确边界是：持有会话时 `start/stop` 可重复全
自动；完全 `shutdown` 后的下一次冷启动可能仍需按一次琉璃蓝牙键。该轮没有播放，
所以不算新的声学或 BASS 证据。

因此本仓库的准确结论是：**厂商 Play Together 控制序列与双响结果已实证；标准
BASS 建链、实际广播源、BIG/BIS 和精确同步误差仍未证明。** 具体原因包括：

- 琉璃 5 的 BASS Receive State 读取没有得到可解释结果；规范要求加密读取，原探测
  未证明满足了该条件；
- BlueZ 缓存中的 DFFD 样本按 App 私有枚举解码为 `RECEIVER(2)`，但新鲜度未知，
  而且它与 OneOS 中 `2=Broadcaster` 不是同一个数字命名空间；
- 没有捕获 LE Audio 周期广播、BASE、BIGInfo 或 ISO 数据。

详见[成功证据与未决问题](docs/EVIDENCE.md)和[协议记录](docs/PROTOCOL.md)。

## 已测试组合

- JBL Authentics 300，固件 `26.24.31.50.00`
- Harman Kardon Aura Studio 5
- JBL One Android `2.7.9`
- Harman Kardon One Android `2.6.11`
- Ubuntu 22.04 / BlueZ 5.64

其他固件、系统和型号均未验证。

## 工作方式

1. 本地轻量管理器先连接琉璃 5，再连接 JBL，并在改变任何角色前持有两条控制会话。
2. JBL 收到 OneOS ENTER 和 `7957 action=1`；琉璃 5 收到 AA `0x3c=ON`。
3. `stop` 不重新连接，而是沿已持有的会话按“琉璃 OFF → JBL `action=2` → JBL
   EXIT”的安全顺序解除。
4. `stop` 后会话保持 `ready`，以后可直接再次 `start`；`shutdown` 才关闭会话并
   恢复先前由工具释放的琉璃 A2DP。

这种方式让两台音响由设备固件协同，而不是让 Linux 同时驱动两个独立时钟的音频
sink；后者在实测中有明显一前一后。

## 快速开始

安装小型运行依赖：

```bash
sudo apt install bluez bluez-tools jq python3 xxd
# PulseAudio 主机还需：sudo apt install pulseaudio-utils
```

先用 `bluetoothctl` 配对并信任两台音响。手机和其他主机应断开琉璃 5，但不用删除
配对。然后把示例配置安装到仓库之外：

```bash
config_path="${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link/devices.env"
install -Dm600 config/devices.env.example "${config_path}"
# 只在 "${config_path}" 中替换两条占位蓝牙地址。

./bin/jbl-aura-link doctor
./bin/jbl-aura-link start
./bin/jbl-aura-link status
```

用 JBL 已支持的任意方式开始播放，并分别听两台音响。解除关联但保留控制会话：

```bash
./bin/jbl-aura-link stop
```

要让 App/其他主机接管或明确结束控制会话时：

```bash
./bin/jbl-aura-link shutdown
```

琉璃也可能在 `shutdown` 后关闭经典蓝牙可连接窗口。如果首次 managed `start` 报
`Host is down`，按一次琉璃蓝牙键并重试。只要管理器仍在 `ready/linked`，真实回归
中的多轮 `start/stop` 都无需再按。以后还想全自动恢复时应使用 `stop`，不要使用
`shutdown`。

v0.3 默认会在 45 秒内每 250 ms 重试一次稀缺的琉璃控制连接。若窗口已经关闭，先
运行 `start`，再在命令等待期间按蓝牙键，电脑就会捕获只有约 2–3 秒的蓝灯窗口；
不再需要靠手工时机碰运气。

## 命令

| 命令 | 作用 |
|---|---|
| `doctor` | 检查依赖、配置、适配器与配对 |
| `start` | 创建或复用持久控制会话，然后关联 |
| `stop` | 通过已持有的会话解除，并保持 `ready` |
| `shutdown` | 解除、关闭两条会话并恢复先前的 A2DP |
| `status` | 显示 managed 状态；离线时只给保守的 DFFD 诊断 |
| `recover-stop` | 没有持久会话时显式执行一次尽力恢复 |
| `frame` | 只离线构造 PL 帧，不接触硬件 |

`start` 在可用时创建一个临时的用户级 systemd 服务。管理器只使用 Python 标准库，
通过权限为 `0600` 的本地 Unix socket 接收命令；runtime/state 目录权限为 `0700`。
默认 `auto` 兼容模式只在建立两条会话时临时卸载 PulseAudio 的
`module-bluetooth-policy` 和 `module-bluetooth-discover`，随后立即恢复它们。控制
会话在模块恢复后仍保持连接。这几秒内其他蓝牙音频会短暂中断。

如果管理器在 `linked` 时被强杀、主机掉电或音响掉电，就不能保证还可新建连接来
自动解除。`recover-stop` 因而明确标为 best-effort；它不会在缺少确认时谎报成功。

真实配置、状态和日志默认都在仓库之外。不要把蓝牙地址、家庭网络地址、抓包、证书、
token、APK、固件或账号材料提交到仓库或公开 issue。

## 文档

- [复现步骤](docs/REPRODUCTION.md)
- [协议记录](docs/PROTOCOL.md)
- [成功证据与未决问题](docs/EVIDENCE.md)
- [已有开源研究](docs/PRIOR_RESEARCH.md)
- [安全与隐私](SECURITY.md)

## 资源占用

实现仅包含 Bash、Python 标准库和 BlueZ `gatttool`。它不加载 CUDA、GPU、模型、音频
解码器、云服务或账号，因此 GPU/显存占用为 **0**；真实回归中的常驻管理器约占十余
MB 内存。

原创代码和文档采用 MIT 许可证。本项目与 JBL、Harman Kardon 及其所有者无隶属或
背书关系；产品名仅用于说明兼容性。公开仓库不分发 APK、固件、反编译源码、账号
材料或厂商秘密。
