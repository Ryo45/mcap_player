use std::fs::File;
// use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread;
use anyhow::{Result, anyhow};
use memmap2::Mmap;
use mcap::MessageStream;


// --- 共通パケット定義 ---
pub struct LoadedPacket {
    pub log_time: u64,
    pub topic: String,
    pub data: Vec<u8>,
}

pub enum ParsedMessage {
    LidarPoints(Vec<LidarPointVertex>),
    CameraImage { width: u32, height: u32, rgba_data: Vec<u8> },
    Unknown,
}

pub struct RenderPacket {
    pub log_time: u64,
    pub topic: String,
    pub payload: ParsedMessage,
}

// --- API ---

pub fn start_input_thread(path: &str) -> Receiver<LoadedPacket> {
    let (tx, rx) = sync_channel(10000); 
    let path_owned = path.to_string();

    thread::spawn(move || {
        // 関数の中でエラーハンドリングを行う
        if let Err(e) = run_native_worker(&path_owned, tx) {
            eprintln!("Input worker failed: {}", e);
        }
    });

    rx
}

// --- Native用ワーカーロジック ---
fn run_native_worker(path: &str, sender: SyncSender<LoadedPacket>) -> Result<()> {
    let file = File::open(path)?;
    
    let mmap = unsafe { Mmap::map(&file)? };
    
    let stream = MessageStream::new(&mmap)?;

    for message_result in stream {
        let message = message_result?;
        
        let packet = LoadedPacket {
            log_time: message.log_time,
            topic: message.channel.topic.clone(),
            data: message.data.into(),
        };

        if sender.send(packet).is_err() {
            break; // 受信側が閉じたら終了
        }
    }
    
    println!("Finished reading MCAP.");
    Ok(())
}

// --- Wasm対応の展望 ---
// 将来、Wasm用のワーカーが必要になったらこう書くだけでOK
// fn run_wasm_worker(url: &str, sender: Sender<LoadedPacket>) -> Result<()> { ... }