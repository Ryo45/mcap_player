use crate::input::LoadedPacket;
use anyhow::{Result, anyhow};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;

// --- 共通定義 ---
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LidarPointVertex {
    pub position: [f32; 3],
    pub intensity: f32,
}

#[derive(Debug)]
pub struct PointCloud2 {
    pub width: u32,
    pub height: u32,
    pub fields: Vec<PointField>,
    pub point_step: u32,
    pub row_step: u32,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct PointField {
    pub name: String,
    pub offset: u32,
    pub datatype: u8,
    pub count: u32,
}

// pub struct LivoxPacket {
//     pub header_seq: u32,
//     pub header_stamp_sec: u32,
//     pub header_stamp_nanosec: u32,
//     pub frame_id: String,
//     pub timebase: u64,
//     pub point_num: u32,
//     pub lidar_id: u8,
//     pub points: Vec<LivoxPoint>,
// }


// --- メインパース関数 ---

pub fn parse_lidar_packet(packet: &LoadedPacket) -> Result<Vec<LidarPointVertex>> {
    if packet.topic.contains("livox") && !packet.topic.contains("pointcloud2") {
        return parse_livox_custom_msg(packet);
    }
    let pcd = parse_pointcloud2(packet)?;
    Ok(pcd_to_vertices(&pcd))
}

// --- Livox CustomMsg (Direct Bytes Read) ---
// アライメント事故を防ぐため、カーソルを手動管理します

fn parse_livox_custom_msg(packet: &LoadedPacket) -> Result<Vec<LidarPointVertex>> {
    let mut cursor = Cursor::new(packet.data.as_slice());
    // let mut cr = CdrReader::new(packet.data.as_slice());

    // 1. Header (Time + FrameId)
    // CDR Header (4) + Time (8) + FrameID (4 + len + \0)
    // 真面目に読むと大変なので、ダンプで判明した固定位置までスキップする手もあるが、
    // ここはまだ CDRReader 的な読み方で進める（文字列があるため）。
    
    // ヘッダ部分だけは可変長なので既存ロジックで読む
    {
        let _seq = read_u32(&mut cursor)?;
        let _sec =  read_u32(&mut cursor)?;
        let _nanosec = read_u32(&mut cursor)?;
        let len = read_u32(&mut cursor)?; // frame_id len
        cursor.set_position(cursor.position() + len as u64); // skip string content
        // 文字列末尾の align 調整が必要な場合があるが、
        // Livoxのframe_idは大抵 "livox_frame" (12文字) など4の倍数なので一旦無視
    }

    // 2. 固定長フィールド
    let _timebase_low = read_u32(&mut cursor)?;
    let _timebase_high = read_u32(&mut cursor)?;
    let point_num = read_u32(&mut cursor)?;
    let _lidar_id = read_u8(&mut cursor)?;
    let _rsvd = read_blob(&mut cursor, 3)?;

    // ★ 現在位置が Points の開始地点
    // ここから先は構造体が詰まっている (Packed) と仮定してガリガリ読む
    
    let mut vertices = Vec::with_capacity(point_num as usize);
    
    for i in 0..point_num {
        // CustomPoint: 19 bytes (or 20 bytes with padding?)
        // uint32 offset_time
        // float x, y, z
        // uint8 reflectivity, tag, line
        
        // 読み込み前の位置を記憶
        let start_pos = cursor.position();

        let offset_time = cursor.read_u32::<LittleEndian>()?;
        let x = cursor.read_f32::<LittleEndian>()?;
        let y = cursor.read_f32::<LittleEndian>()?;
        let z = cursor.read_f32::<LittleEndian>()?;
        let reflectivity = cursor.read_u8()?;
        let _tag = cursor.read_u8()?;
        let _line = cursor.read_u8()?;
        
        // ★デバッグ: 最初の数点だけ生の値を確認
        if i < 3 {
            println!("Point[{}]: off={}, x={:.3}, y={:.3}, z={:.3}", i, offset_time, x, y, z);
        }

        vertices.push(LidarPointVertex {
            position: [x, y, z],
            intensity: reflectivity as f32 / 255.0,
        });

        // アライメント補正があるかもしれないのでチェック
        // 現在 19バイト読んだ。もし次のデータが4バイト境界から始まるなら1バイト飛ばす必要がある。
        // ダンプ解析などから、Livoxは「19バイトPacked」の可能性が高いが、
        // もしズレていくようならここを調整する。
        // cursor.set_position(start_pos + 19); 
    }

    Ok(vertices)
}

// --- ヘルパー関数 (Alignmentなし) ---
fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32> {
    // 4バイト境界への整列を行わず、そのまま読む
    Ok(cursor.read_u32::<LittleEndian>()?)
}
fn read_u8(cursor: &mut Cursor<&[u8]>) -> Result<u8> {
    Ok(cursor.read_u8()?)
}
fn read_blob(cursor: &mut Cursor<&[u8]>, len: u64) -> Result<()> {
    cursor.set_position(cursor.position() + len);
    Ok(())
}

// --- PointCloud2用 (変更なし・省略) ---
// (以前の parse_pointcloud2, pcd_to_vertices, read_f32 をそのまま残してください)
// ※面倒であれば、以前のコードの parse_livox_custom_msg だけ上記に置き換えてください。
// --- 以下、コンパイルを通すためのダミー実装（以前のコードがあるなら不要） ---
fn parse_pointcloud2(packet: &LoadedPacket) -> Result<PointCloud2> {
    // 既存の実装をここに
    Err(anyhow!("Not implemented here, use previous code"))
}
fn pcd_to_vertices(pcd: &PointCloud2) -> Vec<LidarPointVertex> {
    Vec::new()
}