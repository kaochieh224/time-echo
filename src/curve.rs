//! 三段 RGB 曲線 → 256 格 LUT（規格 3.4）。
//!
//! 控制點在亮度 0.25／0.5／0.75，滑桿 ±100 % 對應上下推移 ±0.25；
//! 兩端固定 (0,0)、(1,1)，用單調三次（Fritsch–Carlson）插值，避免過衝。

pub fn build_lut(shadows: f32, mids: f32, highs: f32) -> [u8; 256] {
    let xs = [0.0, 0.25, 0.5, 0.75, 1.0];
    let ys = [
        0.0,
        (0.25 + shadows / 100.0 * 0.25).clamp(0.0, 1.0),
        (0.5 + mids / 100.0 * 0.25).clamp(0.0, 1.0),
        (0.75 + highs / 100.0 * 0.25).clamp(0.0, 1.0),
        1.0,
    ];
    let n = xs.len();
    let d: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) * 0.5 };
    }
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[i] = t * a * d[i];
            m[i + 1] = t * b * d[i];
        }
    }
    let mut lut = [0u8; 256];
    for (k, out) in lut.iter_mut().enumerate() {
        let x = k as f32 / 255.0;
        let i = ((x / 0.25) as usize).min(n - 2);
        let h = xs[i + 1] - xs[i];
        let t = (x - xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * ys[i]
            + (t3 - 2.0 * t2 + t) * h * m[i]
            + (-2.0 * t3 + 3.0 * t2) * ys[i + 1]
            + (t3 - t2) * h * m[i + 1];
        *out = (y.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
    lut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_curve_is_identity() {
        let lut = build_lut(0.0, 0.0, 0.0);
        for (i, v) in lut.iter().enumerate() {
            assert!((*v as i32 - i as i32).abs() <= 1, "{i} → {v}");
        }
    }

    #[test]
    fn demo_curve_lifts_shadows_and_lowers_highlights() {
        let lut = build_lut(43.0, 0.0, -25.0);
        assert!(lut[64] > 64 + 20);
        assert!(lut[191] < 191 - 10);
        assert_eq!(lut[0], 0);
        assert_eq!(lut[255], 255);
        assert!(lut.windows(2).all(|w| w[1] >= w[0]), "曲線應單調");
    }
}
