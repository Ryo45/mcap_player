use std::collections::VecDeque;
use crate::input::LoadedPacket;

pub struct PlayerState{
    pub playing : bool,
    pub current_time_nano : u64,
    pub start_time_nano : u64,
    pub speed_ratio: f64, // 1.0 が等速

    buffer: VecDeque<LoadedPacket>,
}

impl PlayerState{
    pub fn new() -> Self {
        Self {
            playing: false,
            current_time_nano: 0,
            start_time_nano: 0,
            speed_ratio: 1.0,
            buffer: VecDeque::new(),
        }
    }

    pub fn update(
        &mut self,
        dt: f32,
        rx: &std::sync::mpsc::Receiver<LoadedPacket>
    ) -> Vec<LoadedPacket> {
        while let Ok(packet) = rx.try_recv() {
            if self.start_time_nano == 0 {
                self.start_time_nano = packet.log_time;
                self.current_time_nano = packet.log_time;
                self.playing = true; // 自動再生開始
            }
            self.buffer.push_back(packet);
        }

        if !self.playing || self.buffer.is_empty() {
            return Vec::new();
        }

        self.current_time_nano += (dt as f64 * self.speed_ratio * 1_000_000_000.0) as u64;

        let mut packets_to_render = Vec::new();
        
        while let Some(packet) = self.buffer.front() {
            if packet.log_time <= self.current_time_nano {
                if let Some(p) = self.buffer.pop_front() {
                    packets_to_render.push(p);
                }
            } else {
                break;
            }
        }

        packets_to_render
    }
}