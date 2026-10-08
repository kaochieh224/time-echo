//! 環形記憶緩衝的索引計算與分身排列（規格 2.1、2.2、3.2）。
//!
//! 這裡只有純計算，不碰 GPU，方便單元測試；實際影格存在
//! `render::Renderer` 的 texture array，層號由這裡決定。

use crate::params::{Layout, Params};

/// 依緩衝解析度決定的張數上限（規格 2.2）。
pub fn layer_limit(buffer_res: u32) -> usize {
    if buffer_res >= 960 { 120 } else { 240 }
}

/// 實際配置的張數 = min(長度 × 取樣率, 張數上限, GPU 上限)。
pub fn capacity_for(p: &Params, gpu_max_layers: u32) -> usize {
    let wanted = (p.memory_length * p.capture_rate).round().max(1.0) as usize;
    wanted.min(layer_limit(p.buffer_res)).min(gpu_max_layers as usize).max(2)
}

/// 由來源比例與長邊算出緩衝尺寸（偶數）。
pub fn buffer_size(src_w: u32, src_h: u32, long_edge: u32) -> (u32, u32) {
    let (w, h) = (src_w.max(1) as f32, src_h.max(1) as f32);
    let s = long_edge as f32 / w.max(h);
    let even = |v: f32| (((v * s) / 2.0).round() as u32).max(1) * 2;
    (even(w), even(h))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EchoRing {
    capacity: usize,
    /// 最新一張所在的層
    head: usize,
    /// 已存張數（≤ capacity）
    count: usize,
}

impl EchoRing {
    pub fn new(capacity: usize) -> Self {
        EchoRing { capacity: capacity.max(1), head: 0, count: 0 }
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn count(&self) -> usize {
        self.count
    }
    pub fn reset(&mut self) {
        self.head = 0;
        self.count = 0;
    }
    /// 推進一格並回傳要寫入的層號。
    pub fn push(&mut self) -> usize {
        if self.count > 0 {
            self.head = (self.head + 1) % self.capacity;
        }
        self.count = (self.count + 1).min(self.capacity);
        self.head
    }
    /// 往回 `offset` 張的層號；歷史不夠時回傳 None（規格 2.1-5：隱藏，不重複最舊的）。
    pub fn layer_at(&self, offset: usize) -> Option<usize> {
        if offset >= self.count || offset >= self.capacity {
            return None;
        }
        Some((self.head + self.capacity - offset) % self.capacity)
    }
}

/// 第 i 個分身往回的張數：round(i × interval × 取樣率)。
pub fn echo_offset(i: u32, interval: f32, capture_rate: f32) -> usize {
    (i as f32 * interval * capture_rate).round().max(0.0) as usize
}

/// 一個要畫的分身（畫面像素座標，y 向下）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EchoInstance {
    /// 左上、右下
    pub rect: [f32; 4],
    pub layer: u32,
    /// 0 = 最新
    pub index: u32,
    /// Disintegrate 侵蝕量 0–1
    pub erosion: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EchoPlan {
    /// 由舊到新（繪製順序）
    pub instances: Vec<EchoInstance>,
    /// 超出記憶長度（緩衝容量）的分身數
    pub beyond_capacity: u32,
    /// 歷史還不夠而暫時隱藏的分身數
    pub waiting: u32,
    /// 間隔小於一個取樣間隔：相鄰分身會取到同一張
    pub interval_too_small: bool,
}

/// 排出所有分身的位置與層號。
///
/// * `out_w/out_h`：輸出尺寸；`src_w/src_h`：來源（緩衝）比例
/// * `extra_erosion`：節拍脈衝加在 Disintegrate 上的量（0–1）
pub fn plan_echoes(
    p: &Params,
    ring: &EchoRing,
    out_w: f32,
    out_h: f32,
    src_w: f32,
    src_h: f32,
    extra_erosion: f32,
) -> EchoPlan {
    let n = p.echo_count.max(1);
    let interval = p.effective_interval();
    let mut plan = EchoPlan {
        interval_too_small: interval < 1.0 / p.capture_rate,
        ..Default::default()
    };

    // 來源等比塞進輸出 × Figure size，以底部中點為基準。
    let fit = (out_w / src_w).min(out_h / src_h);
    let size = p.figure_size / 100.0;
    let (fw, fh) = (src_w * fit * size, src_h * fit * size);

    let step = if n > 1 { p.spacing / 100.0 * out_w / (n - 1) as f32 } else { 0.0 };
    let anchor = out_w * 0.5 + p.axis_x / 100.0 * out_w * 0.5;
    let erosion_total = (p.disintegrate / 100.0 + extra_erosion).clamp(0.0, 1.0);
    let fade = p.echo_fade / 100.0;

    for i in (0..n).rev() {
        let offset = echo_offset(i, interval, p.capture_rate);
        let Some(layer) = ring.layer_at(offset) else {
            if offset >= ring.capacity() {
                plan.beyond_capacity += 1;
            } else {
                plan.waiting += 1;
            }
            continue;
        };
        let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
        let cx = match p.layout {
            Layout::Procession => anchor - i as f32 * step,
            Layout::Centered => anchor + ((n - 1) as f32 * 0.5 - i as f32) * step,
        };
        plan.instances.push(EchoInstance {
            rect: [cx - fw * 0.5, out_h - fh, cx + fw * 0.5, out_h],
            layer: layer as u32,
            index: i,
            erosion: erosion_total * t,
            opacity: 1.0 - fade * t,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_wraps_and_hides_missing_history() {
        let mut r = EchoRing::new(4);
        assert_eq!(r.layer_at(0), None);
        assert_eq!(r.push(), 0);
        assert_eq!(r.layer_at(0), Some(0));
        assert_eq!(r.layer_at(1), None);
        for expected in [1, 2, 3, 0, 1] {
            assert_eq!(r.push(), expected);
        }
        assert_eq!(r.count(), 4);
        assert_eq!(r.layer_at(0), Some(1));
        assert_eq!(r.layer_at(3), Some(2));
        // 偏移等於張數時會指到剛被覆寫的那格，所以不顯示
        assert_eq!(r.layer_at(4), None);
        r.reset();
        assert_eq!(r.layer_at(0), None);
    }

    #[test]
    fn demo_values_fit_in_default_buffer() {
        // 規格 2.2 驗算：22 分身、0.30 s、15 fps → 最舊偏移 95 張 < 120
        assert_eq!(echo_offset(21, 0.30, 15.0), 95);
        let p = Params::demo();
        assert_eq!(capacity_for(&p, 256), 120);
        let mut ring = EchoRing::new(120);
        for _ in 0..120 {
            ring.push();
        }
        let plan = plan_echoes(&p, &ring, 1280.0, 720.0, 640.0, 360.0, 0.0);
        assert_eq!(plan.instances.len(), 22);
        assert_eq!(plan.beyond_capacity, 0);
    }

    #[test]
    fn capacity_is_clamped() {
        let p = Params { capture_rate: 30.0, memory_length: 16.0, ..Params::default() };
        assert_eq!(capacity_for(&p, 256), 240);
        let p = Params { buffer_res: 960, ..p };
        assert_eq!(capacity_for(&p, 256), 120);
        assert_eq!(capacity_for(&p, 64), 64);
    }

    #[test]
    fn echoes_appear_one_by_one_after_reset() {
        let p = Params { echo_count: 4, interval: 0.2, capture_rate: 10.0, ..Params::default() };
        let mut ring = EchoRing::new(80);
        ring.push();
        let plan = plan_echoes(&p, &ring, 1280.0, 720.0, 640.0, 360.0, 0.0);
        assert_eq!(plan.instances.len(), 1);
        assert_eq!(plan.waiting, 3);
        for _ in 0..2 {
            ring.push();
        }
        // 偏移 0、2 可用（count = 3）
        let plan = plan_echoes(&p, &ring, 1280.0, 720.0, 640.0, 360.0, 0.0);
        assert_eq!(plan.instances.len(), 2);
    }

    #[test]
    fn procession_geometry() {
        // 4 個分身、spacing 100% → 整排寬度 = 輸出寬，相鄰間距 = 寬 / 3
        let p = Params { echo_count: 4, spacing: 100.0, axis_x: 0.0, disintegrate: 100.0, ..Params::default() };
        let mut ring = EchoRing::new(120);
        for _ in 0..120 {
            ring.push();
        }
        let plan = plan_echoes(&p, &ring, 1200.0, 600.0, 640.0, 360.0, 0.0);
        let ins = &plan.instances;
        assert_eq!(ins.len(), 4);
        // 繪製順序：最舊在前，最新在後
        assert_eq!(ins[0].index, 3);
        assert_eq!(ins[3].index, 0);
        let cx = |e: &EchoInstance| (e.rect[0] + e.rect[2]) * 0.5;
        assert!((cx(&ins[3]) - 600.0).abs() < 1e-3);
        // 最舊的往左 3 × 400 px
        assert!((cx(&ins[0]) - (600.0 - 1200.0)).abs() < 1e-3);
        // 腳貼齊底部，size 100% 時高度 = 輸出高
        assert!((ins[3].rect[3] - 600.0).abs() < 1e-3);
        assert!((ins[3].rect[1] - 0.0).abs() < 1e-3);
        // 侵蝕：最新 0、最舊 1
        assert_eq!(ins[3].erosion, 0.0);
        assert!((ins[0].erosion - 1.0).abs() < 1e-6);
    }

    #[test]
    fn spacing_zero_stacks_but_keeps_time_offset() {
        let p = Params { echo_count: 3, spacing: 0.0, ..Params::default() };
        let mut ring = EchoRing::new(120);
        for _ in 0..120 {
            ring.push();
        }
        let plan = plan_echoes(&p, &ring, 1280.0, 720.0, 640.0, 360.0, 0.0);
        let rects: Vec<_> = plan.instances.iter().map(|e| e.rect).collect();
        assert!(rects.windows(2).all(|w| w[0] == w[1]));
        let layers: Vec<_> = plan.instances.iter().map(|e| e.layer).collect();
        assert!(layers.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn buffer_size_follows_source_aspect() {
        assert_eq!(buffer_size(1280, 720, 640), (640, 360));
        assert_eq!(buffer_size(720, 1280, 640), (360, 640));
    }
}
