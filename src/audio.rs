//! 音樂：Import MP3（ffmpeg 解碼）、自動估 BPM、播放／暫停（規格 3.6）。
//!
//! BPM 估計：低頻能量的起音包絡做自相關，找 60–200 BPM 間的峰值。
//! 常見誤差是差兩倍或一半，所以介面另有 ×2／÷2 與 Tap tempo。

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// 解碼用的取樣率與聲道（播放用）
pub const RATE: u32 = 44_100;
pub const CHANNELS: u16 = 2;

/// 解碼成交錯的 f32 立體聲 PCM。
pub fn decode(path: &Path) -> Result<Vec<f32>> {
    let out = Command::new(crate::source::ffmpeg_bin())
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-vn", "-f", "f32le", "-acodec", "pcm_f32le", "-ac", &CHANNELS.to_string(), "-ar", &RATE.to_string(), "-"])
        .output()
        .context("找不到 ffmpeg（請先安裝：brew install ffmpeg）")?;
    if !out.status.success() {
        bail!("讀不了這個音訊檔：{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let samples: Vec<f32> = out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    if samples.is_empty() {
        bail!("檔案裡沒有聲音");
    }
    Ok(samples)
}

/// 由交錯 PCM 估 BPM（回傳 60–200）。訊號太短或沒有節奏時回傳 None。
pub fn estimate_bpm(interleaved: &[f32], channels: usize, rate: u32) -> Option<f32> {
    let hop = 256usize;
    let frames = interleaved.len() / channels;
    if frames < rate as usize * 4 {
        return None;
    }
    // 1. 單聲道 → 一階低通（約 150 Hz）→ 每 hop 的能量
    let alpha = 1.0 - (-2.0 * std::f32::consts::PI * 150.0 / rate as f32).exp();
    let mut lp = 0.0f32;
    let mut energy = Vec::with_capacity(frames / hop + 1);
    let mut acc = 0.0f32;
    for i in 0..frames {
        let mono: f32 = interleaved[i * channels..(i + 1) * channels].iter().sum::<f32>() / channels as f32;
        lp += alpha * (mono - lp);
        acc += lp * lp;
        if (i + 1) % hop == 0 {
            energy.push((acc / hop as f32 + 1e-9).ln());
            acc = 0.0;
        }
    }
    // 2. 起音包絡：對數能量的正向差分，去掉平均
    let mut onset: Vec<f32> = energy.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
    // 平滑（拍長通常不是整數個 hop，起音會落在相鄰兩格）
    let raw = onset.clone();
    for i in 1..onset.len().saturating_sub(1) {
        onset[i] = 0.25 * raw[i - 1] + 0.5 * raw[i] + 0.25 * raw[i + 1];
    }
    let mean = onset.iter().sum::<f32>() / onset.len() as f32;
    onset.iter_mut().for_each(|v| *v -= mean);
    let fps = rate as f32 / hop as f32;

    // 3. 自相關，在 60–200 BPM 範圍找峰值；對 80–160 稍微加權（常見舞曲速度）
    let lag_of = |bpm: f32| 60.0 * fps / bpm;
    let (min_lag, max_lag) = (lag_of(200.0).floor() as usize, lag_of(60.0).ceil() as usize);
    let n = onset.len();
    if n <= max_lag * 2 {
        return None;
    }
    let ac = |lag: usize| -> f32 { (0..n - lag).map(|i| onset[i] * onset[i + lag]).sum::<f32>() / (n - lag) as f32 };
    let mut best = (0.0f32, 0usize);
    for lag in min_lag..=max_lag {
        let bpm = 60.0 * fps / lag as f32;
        let weight = if (80.0..=160.0).contains(&bpm) { 1.0 } else { 0.85 };
        let v = ac(lag) * weight;
        if v > best.0 {
            best = (v, lag);
        }
    }
    if best.1 == 0 || best.0 <= 0.0 {
        return None;
    }
    // 4. 半速修正：週期訊號在兩倍 lag 也有同樣高的峰；太慢且一半 lag 也夠強，就取快的
    let mut l = best.1;
    while 60.0 * fps / l as f32 * 2.0 <= 200.0 && 60.0 * fps / (l as f32) < 90.0 {
        let half = (l as f32 / 2.0).round() as usize;
        let peak = (half.saturating_sub(1)..=half + 1).map(|k| (ac(k), k)).max_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        if half <= min_lag || peak.0 < 0.6 * ac(l) {
            break;
        }
        l = peak.1;
    }
    // 5. 拋物線內插求小數 lag
    let (a, b, c) = (ac(l - 1), ac(l), ac(l + 1));
    let denom = a - 2.0 * b + c;
    let shift = if denom.abs() > 1e-12 { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) } else { 0.0 };
    let bpm = 60.0 * fps / (l as f32 + shift);
    Some((bpm * 10.0).round() / 10.0)
}

pub struct Track {
    pub name: String,
    pub bpm: Option<f32>,
    pub duration: f32,
    #[cfg(feature = "audio")]
    player: Option<rodio::Player>,
    #[cfg(feature = "audio")]
    _sink: Option<rodio::MixerDeviceSink>,
    paused: bool,
}

impl Track {
    /// 載入並開始循環播放。沒有音訊輸出裝置時仍可用 BPM，只是不出聲。
    pub fn load(path: &Path) -> Result<Self> {
        let pcm = decode(path)?;
        let bpm = estimate_bpm(&pcm, CHANNELS as usize, RATE);
        let duration = pcm.len() as f32 / CHANNELS as f32 / RATE as f32;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        #[cfg(feature = "audio")]
        {
            use rodio::Source;
            use std::num::NonZero;
            let (sink, player) = match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut sink) => {
                    sink.log_on_drop(false);
                    let player = rodio::Player::connect_new(sink.mixer());
                    let buf = rodio::buffer::SamplesBuffer::new(NonZero::new(CHANNELS).unwrap(), NonZero::new(RATE).unwrap(), pcm);
                    player.append(buf.repeat_infinite());
                    player.play();
                    (Some(sink), Some(player))
                }
                Err(e) => {
                    eprintln!("沒有音訊輸出：{e}");
                    (None, None)
                }
            };
            Ok(Track { name, bpm, duration, player, _sink: sink, paused: false })
        }
        #[cfg(not(feature = "audio"))]
        Ok(Track { name, bpm, duration, paused: false })
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        #[cfg(feature = "audio")]
        if let Some(p) = &self.player {
            if paused { p.pause() } else { p.play() }
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// 曲內播放位置（s），循環時取餘數；沒有輸出裝置時回傳 None。
    pub fn position(&self) -> Option<f64> {
        #[cfg(feature = "audio")]
        if let Some(p) = &self.player {
            return Some(p.get_pos().as_secs_f64() % self.duration.max(0.001) as f64);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成鼓點：每拍一個 60 Hz 衰減正弦＋一點雜訊
    fn click_track(bpm: f32, seconds: f32, rate: u32) -> Vec<f32> {
        let n = (seconds * rate as f32) as usize;
        let beat = (60.0 / bpm * rate as f32) as usize;
        let mut seed = 1u32;
        (0..n)
            .map(|i| {
                let t = (i % beat) as f32 / rate as f32;
                let kick = (2.0 * std::f32::consts::PI * 60.0 * t).sin() * (-t * 25.0).exp();
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                kick * 0.8 + noise * 0.02
            })
            .collect()
    }

    #[test]
    fn estimates_common_tempos() {
        for bpm in [96.0, 126.0, 140.0] {
            let pcm = click_track(bpm, 20.0, 22_050);
            let est = estimate_bpm(&pcm, 1, 22_050).unwrap();
            assert!((est - bpm).abs() < 1.5, "實際 {bpm}，估計 {est}");
        }
    }

    #[test]
    fn too_short_returns_none() {
        assert_eq!(estimate_bpm(&[0.0; 1000], 1, 22_050), None);
    }
}
