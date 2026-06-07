use std::io::{Cursor, Read, Seek, SeekFrom};
use byteorder::{LittleEndian, ReadBytesExt};
use anyhow::{Result, anyhow};

/// 簡易CDRデコーダー (Little Endian前提)
pub struct CdrReader<'a> {
    cursor: Cursor<&'a [u8]>,
}

impl<'a> CdrReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        // MCAPのROS2メッセージは通常、最初の4バイトがCDRヘッダなどではない場合が多いが、
        // データの配置に従って読む。
        // ※厳密には0バイト目か4バイト目から始まるが、今回は0から読む。
        Self {
            cursor: Cursor::new(data),
        }
    }

    /// 現在位置を align バイト境界に合わせる
    fn align(&mut self, align: u64) -> Result<()> {
        let pos = self.cursor.position();
        if pos % align != 0 {
            let padding = align - (pos % align);
            self.cursor.seek(SeekFrom::Current(padding as i64))?;
        }
        Ok(())
    }

    pub fn read_u8(&mut self) -> Result<u8> {
        // u8はアライメント不要
        Ok(self.cursor.read_u8()?)
    }

    pub fn read_bool(&mut self) -> Result<bool> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        self.align(4)?;
        Ok(self.cursor.read_u32::<LittleEndian>()?)
    }

    pub fn read_f32(&mut self) -> Result<f32> {
        self.align(4)?;
        Ok(self.cursor.read_f32::<LittleEndian>()?)
    }

    pub fn read_string(&mut self) -> Result<String> {
        // 文字列も長さ(u32)の前にアライメントが入る
        let len = self.read_u32()?;
        
        // 文字列の末尾にはnull終端が含まれるため、実際のデータは len だけど
        // RustのStringにするには最後の1バイト(\0)を無視する必要がある場合がある
        // ROS2 CDRの場合は len に \0 が含まれる
        if len <= 1 {
            // 空文字の場合、アライメント調整等は不要だが読み飛ばしは必要かも
            // seekだけして空を返す
             self.cursor.seek(SeekFrom::Current(len as i64))?;
             return Ok(String::new());
        }

        let mut buf = vec![0u8; (len - 1) as usize];
        self.cursor.read_exact(&mut buf)?;
        
        // 終端の\0を読み飛ばす
        self.cursor.seek(SeekFrom::Current(1))?;
        
        Ok(String::from_utf8_lossy(&buf).to_string())
    }
    
    // シーケンス（配列）の長さを読む
    pub fn read_sequence_len(&mut self) -> Result<u32> {
        self.read_u32()
    }

    /// 現在位置から指定バイト数だけ生のバイト列として読み出す（点群データ用）
    pub fn read_blob(&mut self, len: usize) -> Result<Vec<u8>> {
        // u8配列なのでアライメントは不要（ただしシーケンス長の直後なので自然に合う）
        let mut buf = vec![0u8; len];
        self.cursor.read_exact(&mut buf)?;
        Ok(buf)
    }
}