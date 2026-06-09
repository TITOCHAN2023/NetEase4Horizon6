# NetEase FH6 Bridge

把 **网易云音乐** 的声音转成一个本地 HTTP 音频流，喂给《极限竞速：地平线6》的
**Spotify Radio mod** 的「在线电台 / Online Radio」入口，从而在游戏车载电台里听网易云。

> 一句话原理：不是把网易云塞进 mod，而是把网易云**正在播放的声音**通过 mod
> 已有的、官方文档化的「在线电台直链 URL」入口转播进去。

## 它能自动做什么

双击 `netease-fh6-bridge.exe`，它会：

1. **自动把 mod 装进游戏目录** —— 自动探测 Steam/Xbox 的 FH6 安装路径（找不到就问你一次），
   把 mod 复制进去（即 INSTALL.txt 第 3 步的自动化）。装过一次后会记住，之后跳过。
2. **自动起一个本地音频流**，把网易云的声音编码成 MP3 对外提供。
3. **自动打开 mod 控制台网页**，并把流地址**复制到剪贴板**。
4. **自动等待网易云启动并连上** —— 先开桥还是先开网易云都行；网易云暂停时自动补静音，流不断。

你只需要做最后一步：在弹出的网页里选「在线电台」，**Ctrl+V** 粘贴地址、播放。
（这一步无法替你点：见下方“为什么不能全自动”。）

## 重要：本工具不含 mod 本体

mod 的许可证禁止转发 / 重新打包 / 转载它。所以本程序**不内置、也不分发 mod**，
它只会复制**你自己**已经下载并解压好的那份 mod。

正确用法：
1. 从 mod 的**官方渠道**下载并解压 mod（里面有 `version.dll`、`spotify-radio/`、`media/`）。
2. 把 `netease-fh6-bridge.exe` 放进那个解压出来的文件夹。
3. 双击运行。

## 为什么不能“全自动”到一次点击都不用

把流地址自动填进 mod，需要去调用 mod 内部的私有接口——那属于对 mod 的**逆向工程**，
其许可证明确禁止。为守住合规底线，本工具不碰 mod 的内部接口，因此保留了“在网页里粘贴地址”
这最后一步（已帮你复制好，按 Ctrl+V 即可）。除此之外全部自动。

## 为什么这样做是合规的

- ❌ 不读取、不修改、不逆向 mod 的任何代码或私有接口；
- ❌ 不内置/转发 mod 本体；
- ❌ 不破解网易云的加密文件、不绕过任何 DRM——只采集网易云**已解码、正送往扬声器**的声音
  （等同于“录制自己正在播放的音乐”）；
- ✅ mod 这边只用它**官方说明书写明**的「粘贴电台直链 URL」功能。

个人、非商业使用：自己付费听自己的歌，放进自己玩的游戏当背景音乐。

## 系统要求

- Windows 10 版本 2004（build 19041）或更高 / Windows 11（“按进程采集”API 需要）
- 已从官方渠道获取 Spotify Radio mod

## 进游戏后的设置

安装后进游戏：**设置 → 音频**，把 **Radio DJ 设为「关」**、**Streamer Mode 设为「开」**，
否则那个电台站不显示 / 会有 DJ 盖住你的歌。然后把电台切到 mod 那个站即可。

## 命令行用法（进阶）

```
netease-fh6-bridge [命令] [选项]

命令:
  (无) / setup   默认：需要时引导安装 mod，然后开始转播
  install        只把 mod 安装到游戏目录
  run            只开始转播（不尝试安装）
  help           显示帮助

选项:
  --game-dir <路径>  FH6 游戏目录(forzahorizon6.exe 所在文件夹)
  --mod-dir <路径>   mod 源文件夹(含 version.dll/spotify-radio/media)
  --process <名字>   要采集的进程名 (默认 cloudmusic.exe)
  --port <端口>      HTTP 流端口 (默认 8123)
  --bind <地址>      监听地址 (默认 127.0.0.1)
  --lan              等同 --bind 0.0.0.0，允许局域网其它设备访问这个流
  --bitrate <kbps>   MP3 码率 128/160/192/256/320 (默认 192)
  --no-ui            启动后不自动开网页/不复制剪贴板
  -y, --yes          安装时不再询问，直接覆盖
```

设置会保存到 exe 同目录的 `config.toml`，下次自动读取。

## 已知限制

- **延迟**：采集 → MP3 编码 → HTTP 缓冲 → mod 解码，整条链路会有**几秒**延迟。
  当背景音乐没问题；不适合需要和画面精确同步的场景。
- **进程名**：网页版/其它客户端进程名不是 `cloudmusic.exe` 时，用 `--process` 指定。

## 从源码编译

```bash
cargo build --release
# 产物: target/release/netease-fh6-bridge.exe
```

依赖里的 `mp3lame-encoder` 会在构建时编译 LAME 的 C 代码，所以**本地手动编译**需要
C 编译器（Windows 上可用 MSVC，或 MinGW-w64 + GNU 工具链）。
**只想用的人直接下 Releases 里的 exe，不需要任何编译环境。**

仓库的 GitHub Actions（`.github/workflows/release.yml`）会在打 `v*` 标签时自动在
Windows 上编译并把 exe 发布到 Release。

## 技术栈

- [`wasapi`](https://crates.io/crates/wasapi) — 按进程环回采集（Process Loopback）
- [`mp3lame-encoder`](https://crates.io/crates/mp3lame-encoder) — MP3 编码
- [`sysinfo`](https://crates.io/crates/sysinfo) — 查找网易云进程
- 纯 `std` 实现的极简流式 HTTP 服务器

## 许可

MIT，见 [LICENSE](LICENSE)。本工具与网易、Turn 10、Playground Games、微软、Spotify、Apple
均无任何关联。所有商标归各自所有者。Use at your own risk。
