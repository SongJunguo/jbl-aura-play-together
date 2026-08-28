# Linux 下的 JBL 与琉璃 5 Play Together

这是一个非官方、实验性的 Linux 工具，用于自动关联：

- JBL Authentics 300
- Harman Kardon Aura Studio 5（琉璃 5）

公开仓库只做两台音响的 `doctor / start / stop / status`，不包含 Music Assistant、
QQ 音乐、微信登录、家庭 IP、真实 MAC、证书、token、APK、抓包或音频文件。

## 已验证与未证明

2026-08-28，真实设备上完成首次验收：Aura 的 Ubuntu A2DP 已断开，JBL 收到
OneOS `7957`，Aura 接受 AA Play Together ON，用户确认两台都发声。

但仓库不会写成“标准 BASS 已证明建链”，因为：

- Aura 的 BASS Receive State 探测没有得到有效值；BASS 规范要求加密读取，旧探测
  未证明已满足安全条件，因此不能等同于“标准零长度空状态”；
- JBL DFFD 角色字段仍为 `RECEIVER(2)`，不是 `BROADCAST(1)`；
- 没有捕获 LE Audio ISO 流。

准确结论是：**厂商 Play Together 命令序列和双响结果已实证，底层标准实现仍未决。**

## 使用

```bash
sudo apt install bluez bluez-tools jq xxd
config_path="${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link/devices.env"
install -Dm600 config/devices.env.example "${config_path}"
# 在 "${config_path}" 中填写你自己的两个蓝牙 MAC

./bin/jbl-aura-link doctor
./bin/jbl-aura-link start
```

用任意受 JBL 支持的方式先让 JBL 播放音乐，然后实际听两台音响。解除关联：

```bash
./bin/jbl-aura-link stop
```

脚本不会播放音乐，也不会把音频分别推送给两个 Linux sink。真实配置默认放在
仓库之外。

更多信息：

- [复现步骤](docs/REPRODUCTION.md)
- [协议记录](docs/PROTOCOL.md)
- [成功证据与未决问题](docs/EVIDENCE.md)
- [已有开源研究](docs/PRIOR_RESEARCH.md)

本项目不占用 GPU 或显存。
