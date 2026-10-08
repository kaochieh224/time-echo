//! 引擎：把「來源影格＋遮罩 → 擷取 → 排分身 → 算圖」串起來。
//! 桌面程式與離線算圖共用，差別只在時間怎麼推進。

use crate::beat::BeatClock;
use crate::frame::{Frame, Mask};
use crate::memory::{self, EchoPlan, EchoRing};
use crate::params::Params;
use crate::render::{RenderRequest, Renderer};

pub struct Engine {
    pub renderer: Renderer,
    pub ring: EchoRing,
    pub beat: BeatClock,
    /// 距離下次擷取累積的來源時間（s）
    capture_acc: f32,
    src_size: Option<(u32, u32)>,
    /// 緩衝設定（長邊、取樣率、長度）改了但還沒 Reset
    applied_buffer: (u32, f32, f32),
    pub last_plan: EchoPlan,
    pub captured_total: u64,
}

impl Engine {
    pub fn new(renderer: Renderer, p: &Params) -> Self {
        Engine {
            renderer,
            ring: EchoRing::new(2),
            beat: BeatClock::new(),
            capture_acc: f32::MAX,
            src_size: None,
            applied_buffer: (p.buffer_res, p.capture_rate, p.memory_length),
            last_plan: EchoPlan::default(),
            captured_total: 0,
        }
    }

    /// 緩衝設定改了、需要 Reset memory 才會生效。
    pub fn buffer_settings_pending(&self, p: &Params) -> bool {
        self.applied_buffer != (p.buffer_res, p.capture_rate, p.memory_length)
    }

    /// Reset memory：只清緩衝，不重設參數；同時套用新的緩衝設定。
    pub fn reset_memory(&mut self, p: &Params) {
        self.applied_buffer = (p.buffer_res, p.capture_rate, p.memory_length);
        self.ring.reset();
        self.capture_acc = f32::MAX;
        if let Some((w, h)) = self.src_size {
            self.alloc(w, h);
        }
    }

    fn alloc(&mut self, src_w: u32, src_h: u32) {
        let (res, rate, len) = self.applied_buffer;
        let tmp = Params { buffer_res: res, capture_rate: rate, memory_length: len, ..Params::default() };
        let cap = memory::capacity_for(&tmp, self.renderer.max_layers());
        let (bw, bh) = memory::buffer_size(src_w, src_h, res);
        if self.renderer.ensure_memory(bw, bh, cap as u32) || self.ring.capacity() != cap {
            self.ring = EchoRing::new(cap);
        }
    }

    /// 取樣率（已套用的那個）
    pub fn capture_rate(&self) -> f32 {
        self.applied_buffer.1
    }

    /// 緩衝尺寸（分割遮罩也用這個解析度）
    pub fn buffer_dims(&self, src_w: u32, src_h: u32) -> (u32, u32) {
        memory::buffer_size(src_w, src_h, self.applied_buffer.0)
    }

    /// 送進一張新的來源影格。`dt` 是這張距離上一張的來源時間；
    /// 暫停時不要呼叫，擷取就會停止、分身一起凍結（規格 2.1-2）。
    pub fn push_frame(&mut self, p: &Params, frame: &Frame, mask: Option<&Mask>, dt: f32) {
        if self.src_size != Some((frame.width, frame.height)) {
            self.src_size = Some((frame.width, frame.height));
            self.ring.reset();
            self.alloc(frame.width, frame.height);
        }
        self.renderer.upload_source(frame);
        if let Some(m) = mask {
            self.renderer.upload_mask(m);
        }
        let period = 1.0 / self.capture_rate();
        if self.capture_acc == f32::MAX {
            self.capture_acc = period;
        } else {
            self.capture_acc += dt;
        }
        if self.capture_acc >= period {
            self.capture_acc = (self.capture_acc - period).min(period);
            let layer = self.ring.push();
            self.renderer.capture(layer, p);
            self.captured_total += 1;
        }
    }

    /// 只換遮罩（AI 結果晚到時）。
    pub fn update_mask(&mut self, mask: &Mask) {
        self.renderer.upload_mask(mask);
    }

    /// 算一張輸出。`time` 是節拍時鐘用的牆上時間（s）。
    pub fn render(&mut self, p: &Params, time: f64, monitors: bool) -> (u32, u32) {
        let (sw, sh) = self.src_size.unwrap_or((16, 9));
        let long = p.output_res as f32;
        let (ow, oh) = if sw >= sh {
            (long, (long * sh as f32 / sw as f32 / 2.0).round() * 2.0)
        } else {
            ((long * sw as f32 / sh as f32 / 2.0).round() * 2.0, long)
        };
        let pulse = if p.beat_pulse > 0.0 { self.beat.pulse(time, p.bpm) * p.beat_pulse / 100.0 } else { 0.0 };
        let mut sp = p.clone();
        sp.capture_rate = self.capture_rate();
        self.last_plan = memory::plan_echoes(&sp, &self.ring, ow, oh, sw as f32, sh as f32, pulse);

        let fit = (ow / sw as f32).min(oh / sh as f32);
        let (rw, rh) = (sw as f32 * fit / ow, sh as f32 * fit / oh);
        let rect = [0.5 - rw * 0.5, 0.5 - rh * 0.5, 0.5 + rw * 0.5, 0.5 + rh * 0.5];
        self.renderer.render(&RenderRequest {
            params: p,
            echoes: &self.last_plan.instances,
            out_width: ow as u32,
            out_height: oh as u32,
            source_rect_uv: rect,
            time: time as f32,
            monitors: monitors && self.src_size.is_some(),
        });
        (ow as u32, oh as u32)
    }

    pub fn has_source(&self) -> bool {
        self.src_size.is_some()
    }
}
