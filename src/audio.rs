//! 音频采集 + MP3 编码。
//!
//! 用 Windows 的“按进程环回采集”（Process Loopback，Win10 2004+ / Win11）
//! 单独抓取网易云音乐这个进程发出的声音，不碰系统/游戏的其它声音。
//! 抓到的 32-bit float PCM 转成 i16，用 LAME 编码成 MP3，再广播给所有 HTTP 客户端。

use crate::server::{broadcast, Clients};
use anyhow::{anyhow, bail, Result};
use mp3lame_encoder::{max_required_buffer_size, Bitrate, Builder, DualPcm, Encoder, Quality};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;
use wasapi::{
    initialize_mta, AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat,
};

/// 枚举默认播放设备上、当前有音频会话的进程 PID（即“正在/可以出声”的应用）。
/// 用于让用户从“正在出声的应用”里挑一个做音源。
pub fn list_audio_session_pids() -> Vec<u32> {
    let _ = initialize_mta();
    let mut out = Vec::new();
    let device = match DeviceEnumerator::new().and_then(|e| e.get_default_device(&Direction::Render))
    {
        Ok(d) => d,
        Err(_) => return out,
    };
    {
        if let Ok(mgr) = device.get_iaudiosessionmanager() {
            if let Ok(en) = mgr.get_audiosessionenumerator() {
                if let Ok(count) = en.get_count() {
                    for i in 0..count {
                        if let Ok(sess) = en.get_session(i) {
                            if let Ok(pid) = sess.get_process_id() {
                                if pid != 0 && !out.contains(&pid) {
                                    out.push(pid);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// 设环境变量 NFB_DEBUG=1 时打印阶段日志，便于定位问题。
fn dbg(msg: &str) {
    if std::env::var_os("NFB_DEBUG").is_some() {
        eprintln!("[dbg] {msg}");
    }
}

const SAMPLE_RATE: usize = 48_000;
const CHANNELS: usize = 2;
/// 等待音频事件的超时（毫秒）。超时说明网易云此刻没出声（暂停/静音），
/// 这时我们补一段等长的静音，保证 MP3 流不中断，客户端不会掉线。
const WAIT_TIMEOUT_MS: u32 = 300;

/// 采集一次会话：一直运行，直到出错（比如网易云被关闭）才返回。
pub fn capture_session(pid: u32, bitrate_kbps: u32, clients: &Clients) -> Result<()> {
    // COM 必须在本线程初始化为 MTA；已初始化会返回 S_FALSE，忽略即可。
    let _ = initialize_mta();
    dbg("initialize_mta done");

    let mut encoder = build_encoder(bitrate_kbps)?;
    dbg("encoder built");

    let mut client = AudioClient::new_application_loopback_client(pid, true)?;
    dbg("loopback client created");
    let format = WaveFormat::new(32, 32, &SampleType::Float, SAMPLE_RATE, CHANNELS, None);
    // 共享模式 + 事件驱动 + 自动格式转换，缓冲 ~200ms
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 2_000_000, // 200ms（单位 100ns）
    };
    client.initialize_client(&format, &Direction::Capture, &mode)?;
    dbg("client initialized");

    let block_align = format.get_blockalign() as usize;
    if block_align != CHANNELS * 4 {
        bail!("采集格式异常：blockalign={block_align}（期望 {}）", CHANNELS * 4);
    }

    let h_event = client.set_get_eventhandle()?;
    dbg("event handle set");
    let capture = client.get_audiocaptureclient()?;
    dbg("capture client obtained");
    client.start_stream()?;
    dbg("stream started");

    let mut raw: VecDeque<u8> = VecDeque::new();
    let mut left: Vec<i16> = Vec::new();
    let mut right: Vec<i16> = Vec::new();
    let mut first = true;
    // 诊断：累计一段时间的峰值电平
    let mut peak: i32 = 0;
    let mut peak_frames: usize = 0;
    // 实时时钟：保证“产出的样本数”严格跟着真实时间走，避免静音补多了越积越延迟。
    let start = Instant::now();
    let mut emitted: u64 = 0; // 已产出的每声道样本数

    loop {
        match h_event.wait_for_event(WAIT_TIMEOUT_MS) {
            Ok(()) => {
                // 把当前可读的所有数据包都取出来
                loop {
                    match capture.get_next_packet_size()? {
                        Some(0) | None => break,
                        Some(_) => {
                            capture.read_from_device_to_deque(&mut raw)?;
                        }
                    }
                }
                convert(&mut raw, block_align, &mut left, &mut right);
                if first {
                    dbg(&format!("first event: got {} frames", left.len()));
                }
            }
            Err(_) => {
                // 超时（网易云此刻没出声）：只补“真实时间已流逝、但还没产出”的那部分静音，
                // 而不是固定 300ms——否则突发式渲染会让我们补太多静音，造成延迟累积。
                let target = (start.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as u64;
                let deficit = target.saturating_sub(emitted);
                // 单次最多补 0.5s，避免一次塞太多
                let n = deficit.min((SAMPLE_RATE / 2) as u64) as usize;
                if n == 0 {
                    continue;
                }
                left.resize(n, 0);
                right.resize(n, 0);
            }
        }

        if left.is_empty() {
            continue;
        }
        emitted += left.len() as u64;

        // 实时电平：每约 3 秒打印一次，让用户确认确实抓到了网易云的声音。
        for &s in left.iter() {
            let a = (s as i32).abs();
            if a > peak {
                peak = a;
            }
        }
        peak_frames += left.len();
        if peak_frames >= SAMPLE_RATE * 3 {
            let pct = peak as f32 / 327.67;
            if pct < 0.1 {
                println!("🎚  捕获电平 0%（网易云没在出声？被静音？还是没在放歌）");
            } else {
                let bars = (pct / 5.0).round().clamp(1.0, 20.0) as usize;
                println!("🎚  捕获电平 {pct:.0}%  [{}{}]", "#".repeat(bars), "-".repeat(20 - bars));
            }
            peak = 0;
            peak_frames = 0;
        }

        // 按官方文档：先 reserve 足够空间，再 encode 到 spare capacity，最后 set_len。
        let needed = max_required_buffer_size(left.len());
        let mut mp3: Vec<u8> = Vec::with_capacity(needed);
        let n = encoder
            .encode(DualPcm { left: &left, right: &right }, mp3.spare_capacity_mut())
            .map_err(|e| anyhow!("MP3 编码失败: {e:?}"))?;
        // SAFETY: encode 返回写入的字节数 n，且 n <= needed（已 reserve）。
        unsafe {
            mp3.set_len(n);
        }
        if first {
            dbg(&format!("first encode: {n} bytes mp3"));
            first = false;
        }

        left.clear();
        right.clear();

        if !mp3.is_empty() {
            broadcast(clients, Arc::new(mp3));
        }
    }
}

/// 把 VecDeque 里的原始字节（f32 小端、L/R 交错）转成 i16 的左右声道。
/// 只消费完整帧，不足一帧的尾巴留在队列里等下次。
fn convert(raw: &mut VecDeque<u8>, block_align: usize, left: &mut Vec<i16>, right: &mut Vec<i16>) {
    while raw.len() >= block_align {
        let l = pop_f32(raw);
        let r = pop_f32(raw);
        left.push(to_i16(l));
        right.push(to_i16(r));
    }
}

#[inline]
fn pop_f32(raw: &mut VecDeque<u8>) -> f32 {
    // 调用前已保证至少有 block_align(=8) 字节，这里取 4 字节安全
    let b = [
        raw.pop_front().unwrap(),
        raw.pop_front().unwrap(),
        raw.pop_front().unwrap(),
        raw.pop_front().unwrap(),
    ];
    f32::from_le_bytes(b)
}

#[inline]
fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0) as i16
}

fn build_encoder(bitrate_kbps: u32) -> Result<Encoder> {
    let encoder = Builder::new()
        .ok_or_else(|| anyhow!("无法创建 LAME builder"))?
        .with_num_channels(CHANNELS as u8)
        .map_err(|e| anyhow!("set channels: {e:?}"))?
        .with_sample_rate(SAMPLE_RATE as u32)
        .map_err(|e| anyhow!("set sample rate: {e:?}"))?
        .with_brate(bitrate(bitrate_kbps))
        .map_err(|e| anyhow!("set bitrate: {e:?}"))?
        .with_quality(Quality::Best)
        .map_err(|e| anyhow!("set quality: {e:?}"))?
        .build()
        .map_err(|e| anyhow!("build LAME encoder: {e:?}"))?;
    Ok(encoder)
}

fn bitrate(kbps: u32) -> Bitrate {
    match kbps {
        0..=128 => Bitrate::Kbps128,
        129..=160 => Bitrate::Kbps160,
        161..=192 => Bitrate::Kbps192,
        193..=256 => Bitrate::Kbps256,
        _ => Bitrate::Kbps320,
    }
}
