//! 節拍時鐘與 Tap tempo（規格 3.6 的自訂行為）。
//!
//! 原型先不匯入音樂：BPM 手動輸入或連按 Tap 取得，拍點由內部時鐘推算。

pub struct BeatClock {
    /// 第一拍的時間（s）
    origin: f64,
    taps: Vec<f64>,
}

/// 拍點脈衝衰減時間（規格：150 ms）
const PULSE_DECAY: f64 = 0.15;

impl Default for BeatClock {
    fn default() -> Self {
        Self::new()
    }
}

impl BeatClock {
    pub fn new() -> Self {
        BeatClock { origin: 0.0, taps: Vec::new() }
    }

    /// 從 `now` 起重新對拍。
    pub fn restart(&mut self, now: f64) {
        self.origin = now;
    }

    /// 目前在第幾拍、拍內進度 0–1。
    pub fn phase(&self, now: f64, bpm: f32) -> (i64, f64) {
        let beat_len = 60.0 / bpm.max(1.0) as f64;
        let t = (now - self.origin) / beat_len;
        (t.floor() as i64, t - t.floor())
    }

    /// 拍點脈衝：拍點瞬間為 1，150 ms 內線性衰減到 0。
    pub fn pulse(&self, now: f64, bpm: f32) -> f32 {
        let beat_len = 60.0 / bpm.max(1.0) as f64;
        let since = (now - self.origin).rem_euclid(beat_len);
        (1.0 - since / PULSE_DECAY).max(0.0) as f32
    }

    /// 按一下 Tap；連按 4 下以上回傳平均 BPM（60–200）。停頓超過 2 秒重新計。
    pub fn tap(&mut self, now: f64) -> Option<f32> {
        if self.taps.last().is_some_and(|&t| now - t > 2.0) {
            self.taps.clear();
        }
        self.taps.push(now);
        if self.taps.len() > 8 {
            self.taps.remove(0);
        }
        self.origin = now;
        if self.taps.len() < 4 {
            return None;
        }
        let span = self.taps.last().unwrap() - self.taps.first().unwrap();
        let bpm = 60.0 * (self.taps.len() - 1) as f64 / span;
        Some((bpm as f32).clamp(60.0, 200.0))
    }

    pub fn tap_count(&self) -> usize {
        self.taps.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_tempo_averages() {
        let mut c = BeatClock::new();
        let beat = 60.0 / 126.0;
        let mut bpm = None;
        for i in 0..6 {
            bpm = c.tap(10.0 + i as f64 * beat);
        }
        assert!((bpm.unwrap() - 126.0).abs() < 0.01);
    }

    #[test]
    fn tap_resets_after_pause() {
        let mut c = BeatClock::new();
        for i in 0..3 {
            c.tap(i as f64 * 0.5);
        }
        assert_eq!(c.tap(10.0), None);
        assert_eq!(c.tap_count(), 1);
    }

    #[test]
    fn pulse_decays() {
        let c = BeatClock::new();
        assert_eq!(c.pulse(0.0, 120.0), 1.0);
        assert!((c.pulse(0.075, 120.0) - 0.5).abs() < 1e-3);
        assert_eq!(c.pulse(0.2, 120.0), 0.0);
        assert_eq!(c.pulse(0.5, 120.0), 1.0);
        assert_eq!(c.phase(1.25, 120.0).0, 2);
    }
}
