//! 遮罩整理：Shrink / grow 與 Edge softness（規格 3.5）。
//!
//! 在 CPU 上對分割遮罩做，結果才送進擷取 pass，所以只影響之後擷取的影格（規格 2.1-6）。
//! px 以緩衝解析度計，遮罩解析度不同時由 `scale` 換算。

use crate::frame::Mask;

/// `shrink_grow`：負值侵蝕、正值擴張（px）；`softness`：模糊半徑（px）；
/// `scale` = 遮罩寬 ÷ 緩衝寬。
pub fn refine(mask: &mut Mask, shrink_grow: f32, softness: f32, scale: f32) {
    let r = (shrink_grow * scale).round() as i32;
    if r != 0 {
        morph(mask, r.unsigned_abs() as usize, r > 0);
    }
    let s = softness * scale;
    if s > 0.05 {
        blur(mask, s);
    }
}

/// 方形結構元素的 min（侵蝕）或 max（擴張），可分離成水平、垂直兩次。
fn morph(mask: &mut Mask, radius: usize, grow: bool) {
    let (w, h) = (mask.width as usize, mask.height as usize);
    let pick = |a: f32, b: f32| if grow { a.max(b) } else { a.min(b) };
    let mut tmp = vec![0.0; w * h];
    for y in 0..h {
        let row = &mask.data[y * w..(y + 1) * w];
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(radius), (x + radius).min(w - 1));
            tmp[y * w + x] = row[lo..=hi].iter().copied().reduce(pick).unwrap();
        }
    }
    for x in 0..w {
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(radius), (y + radius).min(h - 1));
            mask.data[y * w + x] = (lo..=hi).map(|yy| tmp[yy * w + x]).reduce(pick).unwrap();
        }
    }
}

/// 可分離高斯模糊，sigma = 半徑 ÷ 2。
fn blur(mask: &mut Mask, radius: f32) {
    let sigma = (radius * 0.5).max(0.3);
    let k = radius.ceil() as i32;
    let weights: Vec<f32> = (-k..=k).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = weights.iter().sum();
    let weights: Vec<f32> = weights.iter().map(|w| w / sum).collect();
    let (w, h) = (mask.width as i32, mask.height as i32);
    let mut tmp = vec![0.0; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (j, wt) in weights.iter().enumerate() {
                let xx = (x + j as i32 - k).clamp(0, w - 1);
                acc += wt * mask.data[(y * w + xx) as usize];
            }
            tmp[(y * w + x) as usize] = acc;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (j, wt) in weights.iter().enumerate() {
                let yy = (y + j as i32 - k).clamp(0, h - 1);
                acc += wt * tmp[(yy * w + x) as usize];
            }
            mask.data[(y * w + x) as usize] = acc;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Mask {
        let mut m = Mask::filled(20, 20, 0.0);
        for y in 5..15 {
            for x in 5..15 {
                m.data[y * 20 + x] = 1.0;
            }
        }
        m
    }

    fn area(m: &Mask) -> f32 {
        m.data.iter().sum()
    }

    #[test]
    fn shrink_and_grow_change_area() {
        let mut s = square();
        refine(&mut s, -1.0, 0.0, 1.0);
        assert_eq!(area(&s), 64.0);
        let mut g = square();
        refine(&mut g, 2.0, 0.0, 1.0);
        assert_eq!(area(&g), 196.0);
    }

    #[test]
    fn softness_keeps_area_and_softens_edge() {
        let mut m = square();
        refine(&mut m, 0.0, 2.5, 1.0);
        assert!((area(&m) - 100.0).abs() < 0.5);
        let edge = m.get(5, 10);
        assert!(edge > 0.2 && edge < 0.8, "邊緣應介於 0 與 1：{edge}");
        assert!(m.get(10, 10) > 0.99);
    }

    #[test]
    fn zero_settings_do_nothing() {
        let mut m = square();
        refine(&mut m, 0.0, 0.0, 1.0);
        assert_eq!(m, square());
    }
}
