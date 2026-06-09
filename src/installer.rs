//! 自动把 mod 安装进游戏目录。
//!
//! 重要：本程序**不**内置/分发 mod 本体（那会违反 mod 的许可：禁止转发/重打包）。
//! 它只是把**用户自己已经下载并解压好的** mod 文件，复制进游戏目录，
//! 也就是把 INSTALL.txt 第 3 步“解压到游戏目录”这个动作自动化。
//! 用法：把本 exe 放进解压出来的 mod 文件夹（里面有 version.dll / spotify-radio / media）再运行。

use anyhow::{anyhow, bail, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// mod 安装包应包含的三个东西
const MOD_ITEMS: [&str; 3] = ["version.dll", "spotify-radio", "media"];

/// 在 exe 同目录、其父目录、当前工作目录里找 mod 源（含 version.dll + spotify-radio + media）。
pub fn locate_mod_source() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.to_path_buf());
            if let Some(parent) = dir.parent() {
                candidates.push(parent.to_path_buf());
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    candidates.into_iter().find(|c| is_mod_source(c))
}

fn is_mod_source(dir: &Path) -> bool {
    dir.join("version.dll").is_file()
        && dir.join("spotify-radio").is_dir()
        && dir.join("media").is_dir()
}

/// 检查一个目录像不像 FH6 游戏目录
fn is_game_dir(dir: &Path) -> bool {
    dir.join("forzahorizon6.exe").exists() || dir.join("ForzaHorizon6.exe").exists()
}

/// 扫描常见的 Steam / Xbox / GamePass 安装路径自动定位游戏目录。
pub fn detect_game_dir() -> Option<PathBuf> {
    let rels = [
        r"Steam\steamapps\common\ForzaHorizon6",
        r"SteamLibrary\steamapps\common\ForzaHorizon6",
        r"Program Files (x86)\Steam\steamapps\common\ForzaHorizon6",
        r"Program Files\Steam\steamapps\common\ForzaHorizon6",
        r"Games\Steam\steamapps\common\ForzaHorizon6",
        r"XboxGames\Forza Horizon 6\Content",
        r"Program Files\WindowsApps\Forza Horizon 6\Content",
    ];
    for drive in b'C'..=b'Z' {
        let root = format!("{}:\\", drive as char);
        if !Path::new(&root).exists() {
            continue;
        }
        for rel in rels {
            let p = Path::new(&root).join(rel);
            if is_game_dir(&p) {
                return Some(p);
            }
        }
    }
    None
}

/// 决定最终要安装到的游戏目录：命令行 > 配置 > 自动探测 > 交互询问。
pub fn resolve_game_dir(
    cli: &Option<String>,
    saved: &Option<String>,
    interactive: bool,
) -> Result<PathBuf> {
    if let Some(p) = cli {
        return validate_game_dir(PathBuf::from(p));
    }
    if let Some(p) = saved {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
    }
    if let Some(p) = detect_game_dir() {
        println!("🔎 自动检测到游戏目录：{}", p.display());
        return Ok(p);
    }
    if interactive {
        return prompt_game_dir();
    }
    bail!("未能确定游戏目录。请用 --game-dir \"<游戏目录>\" 指定（forzahorizon6.exe 所在的文件夹）。")
}

fn validate_game_dir(dir: PathBuf) -> Result<PathBuf> {
    if !dir.is_dir() {
        bail!("游戏目录不存在：{}", dir.display());
    }
    if !is_game_dir(&dir) {
        println!(
            "⚠️  注意：在 {} 里没找到 forzahorizon6.exe，目录可能不对，但仍按你的指定继续。",
            dir.display()
        );
    }
    Ok(dir)
}

fn prompt_game_dir() -> Result<PathBuf> {
    loop {
        print!("请输入 Forza Horizon 6 的游戏目录（forzahorizon6.exe 所在文件夹），回车确认：\n> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let line = line.trim().trim_matches('"');
        if line.is_empty() {
            bail!("未输入游戏目录，已取消安装。");
        }
        let pb = PathBuf::from(line);
        match validate_game_dir(pb) {
            Ok(p) => return Ok(p),
            Err(e) => println!("{e} 请重试。"),
        }
    }
}

/// 把 mod 复制进游戏目录。
pub fn install_mod(src: &Path, game: &Path) -> Result<()> {
    for item in MOD_ITEMS {
        let from = src.join(item);
        let to = game.join(item);
        if from.is_dir() {
            copy_dir_all(&from, &to)
                .map_err(|e| anyhow!("复制 {item} 失败：{e}"))?;
        } else if from.is_file() {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::copy(&from, &to).map_err(|e| anyhow!("复制 {item} 失败：{e}"))?;
        } else {
            bail!("mod 源里缺少 {item}（{}）", from.display());
        }
        println!("  ✓ {item}");
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 询问 y/N，默认 No。
pub fn confirm(question: &str) -> bool {
    print!("{question} [y/N]: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
