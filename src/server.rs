//! 极简的 Icecast 风格 HTTP 音频流服务器（纯 std，无第三方 HTTP 依赖）。
//!
//! 每个连进来的客户端（mod 的“在线电台”、浏览器、VLC 等）会注册一个发送端，
//! 采集线程把编码好的 MP3 数据通过 `broadcast` 推给所有客户端。
//! 用 HTTP/1.0 + Connection: close + 无 Content-Length，让客户端一直读到连接关闭，
//! 这是网络电台最通用、兼容性最好的形式。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;

/// 所有已连接客户端的发送端列表。用 Arc<Vec<u8>> 共享同一份编码数据，避免给每个客户端各拷一份。
pub type Clients = Arc<Mutex<Vec<SyncSender<Arc<Vec<u8>>>>>>;

/// 在后台线程里接受连接。
pub fn start(listener: TcpListener, clients: Clients) {
    thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(s) = stream {
                let clients = clients.clone();
                thread::spawn(move || {
                    let _ = handle_client(s, clients);
                });
            }
        }
    });
}

fn handle_client(mut stream: TcpStream, clients: Clients) -> std::io::Result<()> {
    stream.set_nodelay(true).ok();

    // 尽力读掉客户端发来的请求头（我们不关心具体内容，统一当作 GET /stream 处理）。
    let mut buf = [0u8; 2048];
    let _ = stream.read(&mut buf);

    let header = "HTTP/1.0 200 OK\r\n\
                  Content-Type: audio/mpeg\r\n\
                  Cache-Control: no-cache, no-store\r\n\
                  Connection: close\r\n\
                  icy-name: NetEase FH6 Bridge\r\n\
                  \r\n";
    stream.write_all(header.as_bytes())?;

    // 注册到广播列表。容量小，既防内存撑爆，也把我们这侧的额外延迟上限压低
    // （慢客户端时宁可丢新块也不积压，保持“接近实时”）。
    let (tx, rx) = sync_channel::<Arc<Vec<u8>>>(64);
    clients.lock().unwrap().push(tx);

    // 持续把收到的 MP3 数据写给这个客户端，写失败（客户端断开）就退出。
    for chunk in rx {
        if stream.write_all(&chunk).is_err() {
            break;
        }
    }
    // 退出后 rx 被丢弃，采集线程下次 try_send 会得到 Disconnected，从而把这个客户端清掉。
    Ok(())
}

/// 把一段编码好的 MP3 数据推给所有客户端，顺便清理已断开的客户端。
pub fn broadcast(clients: &Clients, chunk: Arc<Vec<u8>>) {
    let mut guard = clients.lock().unwrap();
    guard.retain(|tx| match tx.try_send(chunk.clone()) {
        Ok(()) => true,
        // 客户端太慢、缓冲满了：丢掉这一帧但保留连接（避免无限堆积）
        Err(TrySendError::Full(_)) => true,
        // 客户端已断开：移除
        Err(TrySendError::Disconnected(_)) => false,
    });
}
