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

## 已测试组合

- JBL Authentics 300，固件 `26.24.31.50.00`
- Harman Kardon Aura Studio 5
- JBL One Android `2.7.9`
- Harman Kardon One Android `2.6.11`
- Ubuntu 22.04 / BlueZ 5.64

其他固件、系统和型号均未验证。

## 工作方式

1. 本地轻量管理器从实时 FDDF 广告解析琉璃当前随机 LE 地址（经典地址保留为兼容
   回退），再连接 JBL，并在改变任何角色前持有两条控制会话。
2. JBL 收到 OneOS ENTER 和 `7957 action=1`；琉璃 5 收到 AA `0x3c=ON`。
3. `stop` 不重新连接，而是沿已持有的会话按“琉璃 OFF → JBL `action=2` → JBL
   EXIT”的安全顺序解除。
4. `stop` 后会话保持 `ready`，以后可直接再次 `start`；`shutdown` 关闭会话，再有界地
   尝试恢复先前由工具释放的琉璃 A2DP。A2DP 恢复失败会被明确报告，但不会继续扣住
   厂商控制会话。

这种方式让两台音响由设备固件协同，而不是让 Linux 同时驱动两个独立时钟的音频
sink；后者在实测中有明显一前一后。

## 快速开始

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
- [已有开源研究](docs/PRIOR_RESEARCH.md)
- [安全与隐私](SECURITY.md)
- [版本记录](CHANGELOG.md)

## 资源占用

实现仅包含 Bash、Python、`dbus-fast` 和 BlueZ `gatttool`。它不加载 CUDA、GPU、
模型、音频解码器、云服务或账号，因此 GPU/显存占用为 **0**；真实回归中的常驻
管理器约占十余 MB 内存。

原创代码和文档采用 MIT 许可证。本项目与 JBL、Harman Kardon 及其所有者无隶属或
背书关系；产品名仅用于说明兼容性。公开仓库不分发 APK、固件、反编译源码、账号
材料或厂商秘密。
