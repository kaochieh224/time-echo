//! 05 Keylight：色鍵遮罩（Screen colour／gain／balance）與影片自帶 alpha。
//!
//! 演算法是自訂的「Keylight 風格」，不是 Foundry Keylight：
//! * 有彩度的幕（綠幕、藍幕）：色差法。幕色主通道 p，其餘 q、r；
//!   畫素的「幕值」= C[p] − (balance·C[q] + (1−balance)·C[r])，與幕色本身的幕值相比，
//!   越接近越透明。Gain 放大排除強度。
//! * 中性幕（黑、白、灰）：用亮度差與 RGB 距離的加權（balance 決定比重）。
//! Despill 修正前景 RGB，在 GPU 擷取 pass 做（`post.wgsl` 的 fs_capture）。

use crate::frame::{Frame, Mask};

/// 幕色是否接近中性色（飽和度低）
pub fn is_neutral(screen: [f32; 3]) -> bool {
    let max = screen.iter().cloned().fold(0.0, f32::max);
    let min = screen.iter().cloned().fold(1.0, f32::min);
    max - min < 0.15
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// 單一畫素的 alpha（1 = 保留前景）。`gain`、`balance` 為 0–2、0–1。
pub fn key_alpha(c: [f32; 3], screen: [f32; 3], gain: f32, balance: f32) -> f32 {
    if is_neutral(screen) {
        let dl = (luma(c) - luma(screen)).abs();
        let dc = ((c[0] - screen[0]).powi(2) + (c[1] - screen[1]).powi(2) + (c[2] - screen[2]).powi(2)).sqrt() / 3f32.sqrt();
        let d = balance * dl + (1.0 - balance) * dc;
        // gain 100 % 時，距離 0.25 以上完全保留
        return (d * 4.0 * gain).clamp(0.0, 1.0);
    }
    let p = (0..3).max_by(|&a, &b| screen[a].total_cmp(&screen[b])).unwrap();
    let (q, r) = ((p + 1) % 3, (p + 2) % 3);
    let sv = |x: [f32; 3]| x[p] - (balance * x[q] + (1.0 - balance) * x[r]);
    let screen_sv = sv(screen).max(0.05);
    let ratio = (sv(c) / screen_sv).max(0.0);
    (1.0 - ratio * gain).clamp(0.0, 1.0)
}

/// 整張遮罩（`w × h`，通常是緩衝解析度）。
pub fn key_mask(frame: &Frame, w: u32, h: u32, screen: [f32; 3], gain: f32, balance: f32) -> Mask {
    let mut m = Mask::filled(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let c = frame.sample_rgb((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            m.data[(y * w + x) as usize] = key_alpha(c, screen, gain, balance);
        }
    }
    m
}

/// 影片自帶 alpha（MOV ProRes 4444、WebM VP9 alpha）。
pub fn alpha_mask(frame: &Frame, w: u32, h: u32) -> Mask {
    let mut m = Mask::filled(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let sx = ((x as f32 + 0.5) / w as f32 * frame.width as f32) as u32;
            let sy = ((y as f32 + 0.5) / h as f32 * frame.height as f32) as u32;
            let i = ((sy.min(frame.height - 1) * frame.width + sx.min(frame.width - 1)) * 4 + 3) as usize;
            m.data[(y * w + x) as usize] = frame.rgba[i] as f32 / 255.0;
        }
    }
    m
}

/// 兩張遮罩相乘（AI × Key）；尺寸需相同。
pub fn multiply(a: &mut Mask, b: &Mask) {
    for (x, y) in a.data.iter_mut().zip(&b.data) {
        *x *= y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: [f32; 3] = [0.0, 0.69, 0.25];

    #[test]
    fn green_screen_is_removed_and_skin_kept() {
        assert!(key_alpha(GREEN, GREEN, 1.0, 0.5) < 0.01);
        assert!(key_alpha([0.85, 0.65, 0.55], GREEN, 1.0, 0.5) > 0.99, "膚色應保留");
        // 偏綠的邊緣半透明
        let edge = key_alpha([0.3, 0.55, 0.3], GREEN, 1.0, 0.5);
        assert!(edge > 0.1 && edge < 0.9, "{edge}");
        // 提高 gain 移除更多
        assert!(key_alpha([0.3, 0.55, 0.3], GREEN, 1.5, 0.5) < edge);
    }

    #[test]
    fn black_screen_keys_by_brightness() {
        let black = [0.0; 3];
        assert!(is_neutral(black));
        assert!(key_alpha([0.02, 0.02, 0.02], black, 1.0, 0.9) < 0.1);
        assert!(key_alpha([0.8, 0.7, 0.6], black, 1.0, 0.9) > 0.99);
    }

    #[test]
    fn alpha_channel_becomes_mask() {
        let f = Frame::new(2, 1, vec![255, 0, 0, 0, 0, 255, 0, 255]);
        let m = alpha_mask(&f, 2, 1);
        assert_eq!(m.data, vec![0.0, 1.0]);
    }
}
