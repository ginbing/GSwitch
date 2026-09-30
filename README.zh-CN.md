# GSwitch

简体中文 · [English](./README.md)

在一台电脑上管理多个 Codex 账户：看每个账户还剩多少额度，一键切换，接着用 Codex。

![使用六个示例账户的 GSwitch 中文主界面](./assets/readme-demo-zh-CN.png)

- **额度一眼看全。** 每个账户的 5 小时和每周额度、重置时间、可用的重置额度。
- **切换可靠。** 退出 Codex 后点“切换”。GSwitch 写入后会核对新账户，核对失败就恢复原来的登录。
- **从 Cockpit Tools 搬过来。** 扫描本机的 [Cockpit Tools](https://github.com/jlcodes99/cockpit-tools)
  账户，或导入它导出的文件，勾选后导入。
- **登录失效就地重新登录。** 在账户卡片上重新登录，不用删掉重加。
- **唤醒。** 给选中的账户各发一条很短的 Codex 请求，看哪些账户有回复。
- **没有统计和遥测。** 保存的凭据加密存放在本机。

## 安装

从[最新版本](https://github.com/ginbing/GSwitch/releases/latest)下载：

- **Windows：** `GSwitch_<版本>_x64-setup.exe`。安装包没有代码签名，SmartScreen 可能会提示。
  GSwitch 需要 WebView2 运行时，Windows 11 和较新的 Windows 10 自带。如果安装时卡在下载
  WebView2，先从[微软官网](https://developer.microsoft.com/microsoft-edge/webview2/)安装，再运行安装包。
- **macOS：** Apple 芯片选 `aarch64.dmg`，Intel 选 `x64.dmg`。没有经过 Apple 公证，首次打开需要在
  “系统设置 → 隐私与安全性”里允许。
- **Linux：** AppImage 或 `.deb` 安装包。

GSwitch 会自己检查更新。

卸载：Windows 在“设置 → 应用 → 已安装的应用”中卸载；macOS 把 GSwitch 移到废纸篓；Linux 删除
AppImage，或用包管理器卸载 `.deb` 安装包。

## 添加账户

- **登录：** 使用 Codex 官方的浏览器登录。
- **在此电脑上查找：** 点击扫描后，GSwitch 读取 Codex 当前登录的账户，以及 Cockpit Tools 的本地账户
  （默认在 `~/.antigravity_cockpit`，装在别处可以选择文件夹）。先预览，勾选后才导入；已在 GSwitch
  中的账户不会被覆盖。
- **选择文件：** Cockpit Tools 导出的文件、Codex 的 `auth.json`、Sub2API 或 CPA 的导出，可以一次选多个。

GSwitch 不会修改 Cockpit Tools 的任何文件，只带走登录所需的凭据；密码、2FA 密钥、备注和标签留在
Cockpit Tools 里。

## 切换之前

- 先安装 Codex。
- 退出 Codex，包括桌面版、CLI 和编辑器插件。Codex 还在运行时 GSwitch 会提示你，但不会替你关掉它。
- Codex 需要把登录保存在文件里。如果不是，GSwitch 会提示你开启。

## 数据和联网

- 保存的凭据加密存放在本机，密钥由系统的凭据管理器保管。
- GSwitch 只连接 OpenAI（额度和账户操作）和 GitHub（检查 GSwitch 更新和 Codex CLI 新版本）。

## 从源码构建

先安装对应平台的 [Tauri 前置依赖](https://tauri.app/start/prerequisites/)，然后运行：

```bash
git clone https://github.com/ginbing/GSwitch.git
cd GSwitch
pnpm install
pnpm tauri dev
```

开发文档从 [docs/README.md](./docs/README.md) 开始。

## 许可证

[AGPL-3.0](./LICENSE)
