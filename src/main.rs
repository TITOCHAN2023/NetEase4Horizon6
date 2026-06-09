//! NetEase FH6 Bridge
//!
//! 把网易云音乐的声音转成本地 HTTP 音频流，喂给 Forza Horizon 6 的
//! Spotify Radio mod 的「在线电台 / Online Radio」入口，从而在游戏车载电台里听网易云。
//!
//! 设计原则（合规）：本工具是完全独立的程序，
//! - 不读取、不修改、不逆向那个 mod 的任何代码；只用它文档化的「在线电台 URL」入口；
//! - 不内置/转发 mod 本体（许可禁止重打包/转发）；安装功能只是把用户**自己**下载解压好的
//!   mod 文件复制进游戏目录（即 INSTALL.txt 第 3 步的自动化）。

mod audio;
mod config;
mod installer;
mod server;

use anyhow::{anyhow, Result};
use config::Config;
use std::collections::HashSet;
use std::io::Write;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use sysinfo::System;

#[derive(Default)]
struct Opts {
    game_dir: Option<String>,
    mod_dir: Option<String>,
    process: Option<String>,
    bind: Option<String>,
    port: Option<u16>,
    bitrate: Option<u32>,
    yes: bool,
    no_ui: bool,
}

/// 让中文版 Windows 控制台用 UTF-8 显示，避免输出乱码。
#[cfg(windows)]
fn enable_utf8_console() {
    extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn SetConsoleCP(code_page: u32) -> i32;
    }
    const CP_UTF8: u32 = 65001;
    unsafe {
        SetConsoleOutputCP(CP_UTF8);
        SetConsoleCP(CP_UTF8);
    }
}

#[cfg(not(windows))]
fn enable_utf8_console() {}

fn main() -> Result<()> {
    enable_utf8_console();
    print_banner();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .cloned()
        .unwrap_or_default();
    let opts = parse_opts(&args);

    let first_run = !config::config_exists();
    let mut cfg = Config::load();
    apply_opts(&mut cfg, &opts);

    match mode.as_str() {
        "help" => {
            print_help();
            Ok(())
        }
        "install" => {
            let installed = install_flow(&mut cfg, &opts, true, false)?;
            cfg.save();
            if installed {
                println!("\n✅ 安装完成。下次双击本程序即可直接开始转播。");
            }
            pause();
            Ok(())
        }
        "run" => {
            if opts.process.is_none() {
                pick_process_with_timeout(&mut cfg);
            }
            cfg.save();
            run_bridge(&cfg, effective_open_ui(&cfg, &opts))
        }
        // 选音源（随时可重新选）
        "pick" => {
            pick_process_with_timeout(&mut cfg);
            cfg.save();
            println!("已保存音源：{}（下次启动沿用）", cfg.process_name);
            Ok(())
        }
        // 默认（双击 / setup）：首次启动问游戏目录并装 mod；之后需要时再装。然后选音源、转播。
        "" | "setup" => {
            if first_run {
                let _ = install_flow(&mut cfg, &opts, true, true);
            } else {
                maybe_install(&mut cfg, &opts);
            }
            if opts.process.is_none() {
                pick_process_with_timeout(&mut cfg);
            }
            cfg.save();
            run_bridge(&cfg, effective_open_ui(&cfg, &opts))
        }
        other => {
            eprintln!("未知命令：{other}\n");
            print_help();
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// 安装流程
// ---------------------------------------------------------------------------

/// 默认模式下：若尚未安装且找得到 mod 源，就引导安装（非致命，失败也继续转播）。
fn maybe_install(cfg: &mut Config, opts: &Opts) {
    let already = cfg
        .game_dir
        .as_ref()
        .map(|g| Path::new(g).join("version.dll").exists())
        .unwrap_or(false);
    if already {
        return;
    }
    if installer::locate_mod_source().is_none() && opts.mod_dir.is_none() {
        // 不在 mod 文件夹里运行，跳过安装，直接当纯转播工具用
        return;
    }
    if let Err(e) = install_flow(cfg, opts, true, false) {
        println!("⚠️  自动安装跳过：{e}");
    }
}

/// 返回 true 表示真的执行了安装。
/// force_prompt_gamedir=true 时（首次启动）一定会问/确认游戏目录。
fn install_flow(
    cfg: &mut Config,
    opts: &Opts,
    interactive: bool,
    force_prompt_gamedir: bool,
) -> Result<bool> {
    let src = match opts.mod_dir.as_ref().map(Into::into).or_else(installer::locate_mod_source) {
        Some(s) => s,
        None => {
            println!(
                "ℹ️  未找到 mod 安装包（需要 version.dll + spotify-radio + media）。\n   \
                 请把本程序放进你解压好的 mod 文件夹再运行，或用 --mod-dir 指定其位置。\n   \
                 （跳过安装，仍可作为纯转播工具使用。）"
            );
            return Ok(false);
        }
    };

    let game = if force_prompt_gamedir {
        installer::first_run_game_dir(&opts.game_dir, &cfg.game_dir)?
    } else {
        installer::resolve_game_dir(&opts.game_dir, &cfg.game_dir, interactive)?
    };

    if !opts.yes
        && !installer::confirm(&format!(
            "将把 mod 安装/覆盖到游戏目录：\n    {}\n继续即表示你已阅读并接受该 mod 的 LICENSE。是否继续？",
            game.display()
        ))
    {
        println!("已取消安装。");
        return Ok(false);
    }

    println!("📦 正在安装 mod 到游戏目录……");
    installer::install_mod(&src, &game)?;
    cfg.game_dir = Some(game.to_string_lossy().into_owned());

    println!("✅ mod 安装完成。");
    println!("⚙️  别忘了进游戏后：设置 -> 音频，设置 Radio DJ = 关，Streamer Mode = 开。");
    Ok(true)
}

// ---------------------------------------------------------------------------
// 转播主流程
// ---------------------------------------------------------------------------

/// 本次运行是否启用自动开网页/剪贴板：配置里开着、且本次没传 --no-ui。
/// （--no-ui 只影响本次，不持久化。）
fn effective_open_ui(cfg: &Config, opts: &Opts) -> bool {
    cfg.open_ui && !opts.no_ui
}

fn run_bridge(cfg: &Config, open_ui: bool) -> Result<()> {
    let clients: server::Clients = Arc::new(Mutex::new(Vec::new()));
    let addr = format!("{}:{}", cfg.bind, cfg.port);
    let listener = TcpListener::bind(&addr)
        .map_err(|e| anyhow!("无法监听 {addr}：{e}（端口被占用？用 --port 换一个）"))?;
    server::start(listener, clients.clone());

    let url = format!("http://{}:{}/stream", display_host(&cfg.bind), cfg.port);

    println!();
    println!("✅ 本地音频流已就绪：");
    println!("      {url}");
    println!();

    if open_ui {
        if copy_to_clipboard(&url) {
            println!("📋 已把这个地址复制到剪贴板。");
        }
        let mod_ui = format!("http://localhost:{}", cfg.mod_ui_port);
        if open_url(&mod_ui) {
            println!("🌐 已打开 mod 控制台：{mod_ui}");
        }
        println!();
        println!("👉 在打开的网页里选「在线电台 / Online Radio」，把地址粘贴进去（Ctrl+V）并播放。");
    }

    // 检测不到网易云就尝试自动把它启动起来（仅当音源是网易云时）
    if cfg.autostart_netease && cfg.process_name.eq_ignore_ascii_case("cloudmusic.exe") {
        let mut sys = System::new_all();
        if find_main_pid(&sys, &cfg.process_name).is_none() {
            if let Some(path) = locate_netease(cfg) {
                println!("🚀 没检测到网易云，正在自动启动：{}", path.display());
                let _ = Command::new(&path).spawn();
                std::thread::sleep(Duration::from_millis(500));
                sys.refresh_all();
            } else {
                println!("ℹ️  没检测到网易云，也没找到它的安装位置。请手动打开网易云音乐，或在 config.toml 里设置 netease_path。");
            }
        }
    }

    println!();
    println!("⏳ 等待网易云音乐（{}）启动……（按 Ctrl+C 退出）", cfg.process_name);
    println!();

    let mut sys = System::new_all();
    loop {
        let pid = wait_for_process(&mut sys, &cfg.process_name);
        println!("🎵 已连接到网易云音乐 (PID {pid})，开始转播。现在在网易云里放歌即可。");
        if let Err(e) = audio::capture_session(pid, cfg.bitrate_kbps, &clients) {
            println!("⚠️  采集结束/中断：{e}");
        }
        println!("🔌 已断开，重新等待网易云音乐……");
    }
}

/// 阻塞直到找到目标进程，返回它的主进程 PID。
fn wait_for_process(sys: &mut System, name: &str) -> u32 {
    loop {
        sys.refresh_all();
        if let Some(pid) = find_main_pid(sys, name) {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(1500));
    }
}

/// 在同名进程里挑出“树根”那个（父进程不是同名进程的），配合 include_tree=true
/// 覆盖网易云的全部子进程。
fn find_main_pid(sys: &System, name: &str) -> Option<u32> {
    let matches: Vec<(u32, Option<u32>)> = sys
        .processes()
        .iter()
        .filter(|(_, p)| p.name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|(pid, p)| (pid.as_u32(), p.parent().map(|pp| pp.as_u32())))
        .collect();

    if matches.is_empty() {
        return None;
    }

    let set: HashSet<u32> = matches.iter().map(|(pid, _)| *pid).collect();
    matches
        .iter()
        .find(|(_, parent)| parent.map_or(true, |pp| !set.contains(&pp)))
        .or_else(|| matches.first())
        .map(|(pid, _)| *pid)
}

/// 列出当前正在出声的应用，让用户输入序号选择音源；10 秒不选则保持默认（网易云）。
/// 命令行给了 --process 时不会进这里。
fn pick_process_with_timeout(cfg: &mut Config) {
    let pids = audio::list_audio_session_pids();
    let mut sys = System::new_all();
    sys.refresh_all();

    // pid -> 进程名，按名字去重
    let mut names: Vec<String> = Vec::new();
    for pid in &pids {
        if let Some((_, p)) = sys.processes().iter().find(|(k, _)| k.as_u32() == *pid) {
            let n = p.name().to_string_lossy().into_owned();
            if !names.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                names.push(n);
            }
        }
    }

    if names.is_empty() {
        println!("（未检测到正在出声的应用，使用默认音源：{}）", cfg.process_name);
        return;
    }

    println!();
    println!("🎧 检测到正在出声的应用，输入序号选择音源：");
    for (i, n) in names.iter().enumerate() {
        let mark = if n.eq_ignore_ascii_case(&cfg.process_name) {
            "   ← 默认"
        } else {
            ""
        };
        println!("   [{}] {}{}", i + 1, n, mark);
    }
    println!("（输入序号回车选择；10 秒内不选 → 默认用「{}」；也可直接输入进程名）", cfg.process_name);
    print!("> ");
    std::io::stdout().flush().ok();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        if std::io::stdin().read_line(&mut s).is_ok() {
            let _ = tx.send(s);
        }
    });

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(line) => {
            let t = line.trim();
            if let Ok(idx) = t.parse::<usize>() {
                if (1..=names.len()).contains(&idx) {
                    cfg.process_name = names[idx - 1].clone();
                    println!("✓ 已选择音源：{}", cfg.process_name);
                    return;
                }
            }
            if !t.is_empty() {
                cfg.process_name = t.to_string();
                println!("✓ 已选择音源：{}", cfg.process_name);
                return;
            }
            println!("（使用默认音源：{}）", cfg.process_name);
        }
        Err(_) => {
            println!("\n⏱ 10 秒未选择，使用默认音源：{}", cfg.process_name);
        }
    }
}

/// 找到网易云音乐 exe：优先用配置里的路径，否则探测常见安装位置。
fn locate_netease(cfg: &Config) -> Option<std::path::PathBuf> {
    if let Some(p) = &cfg.netease_path {
        let pb = std::path::PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let mut candidates = vec![
        r"C:\Program Files\NetEase\CloudMusic\cloudmusic.exe".to_string(),
        r"C:\Program Files (x86)\NetEase\CloudMusic\cloudmusic.exe".to_string(),
        r"C:\Program Files (x86)\Netease\CloudMusic\cloudmusic.exe".to_string(),
    ];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            Path::new(&local)
                .join(r"Netease\CloudMusic\cloudmusic.exe")
                .to_string_lossy()
                .into_owned(),
        );
    }
    candidates
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.is_file())
}

// ---------------------------------------------------------------------------
// 小工具：剪贴板、打开浏览器
// ---------------------------------------------------------------------------

fn copy_to_clipboard(text: &str) -> bool {
    let Ok(mut child) = Command::new("clip").stdin(Stdio::piped()).spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

fn open_url(url: &str) -> bool {
    // cmd 的 start：第一个引号参数是窗口标题，要留空
    Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .is_ok()
}

// ---------------------------------------------------------------------------
// 参数解析 & 帮助
// ---------------------------------------------------------------------------

fn parse_opts(args: &[String]) -> Opts {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--game-dir" => o.game_dir = it.next().cloned(),
            "--mod-dir" => o.mod_dir = it.next().cloned(),
            "--process" => o.process = it.next().cloned(),
            "--bind" => o.bind = it.next().cloned(),
            "--port" => o.port = it.next().and_then(|v| v.parse().ok()),
            "--bitrate" => o.bitrate = it.next().and_then(|v| v.parse().ok()),
            "--lan" => o.bind = Some("0.0.0.0".into()),
            "-y" | "--yes" => o.yes = true,
            "--no-ui" => o.no_ui = true,
            "-h" | "--help" => {} // mode 已处理
            _ => {}
        }
    }
    o
}

fn apply_opts(cfg: &mut Config, o: &Opts) {
    if let Some(v) = &o.process {
        cfg.process_name = v.clone();
    }
    if let Some(v) = &o.bind {
        cfg.bind = v.clone();
    }
    if let Some(v) = o.port {
        cfg.port = v;
    }
    if let Some(v) = o.bitrate {
        cfg.bitrate_kbps = v;
    }
    // 注意：--no-ui 不在此持久化，只作用于本次运行（见 effective_open_ui）。
}

fn display_host(bind: &str) -> String {
    if bind == "0.0.0.0" {
        "localhost".into()
    } else {
        bind.to_string()
    }
}

fn pause() {
    print!("\n按回车退出……");
    std::io::stdout().flush().ok();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}

fn print_banner() {
    println!("============================================");
    println!(" NetEase FH6 Bridge  v{}", env!("CARGO_PKG_VERSION"));
    println!(" 网易云音乐 -> 地平线6 在线电台 桥接");
    println!("============================================");
}

fn print_help() {
    println!();
    println!("用法: netease-fh6-bridge [命令] [选项]");
    println!();
    println!("命令:");
    println!("  (无)        默认：需要时引导安装 mod，然后开始转播");
    println!("  setup       同上");
    println!("  install     只把 mod 安装到游戏目录");
    println!("  run         只开始转播（不尝试安装）");
    println!("  pick        重新选择音源（列出正在出声的应用）");
    println!("  help        显示本帮助");
    println!();
    println!("选项:");
    println!("  --game-dir <路径>  FH6 游戏目录(forzahorizon6.exe 所在文件夹)");
    println!("  --mod-dir <路径>   mod 源文件夹(含 version.dll/spotify-radio/media)");
    println!("  --process <名字>   要采集的进程名 (默认 cloudmusic.exe)");
    println!("  --port <端口>      HTTP 流端口 (默认 8123)");
    println!("  --bind <地址>      监听地址 (默认 127.0.0.1)");
    println!("  --lan              等同 --bind 0.0.0.0，允许局域网设备访问");
    println!("  --bitrate <kbps>   MP3 码率 128/160/192/256/320 (默认 192)");
    println!("  --no-ui            启动后不自动开网页/不复制剪贴板");
    println!("  -y, --yes          安装时不再询问，直接覆盖");
    println!("  -h, --help         显示本帮助");
    println!();
    println!("设置会保存到 exe 同目录的 config.toml，下次自动读取。");
}
