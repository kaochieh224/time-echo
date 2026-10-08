//! CPU 端的影格與遮罩容器。

/// RGBA8 影格（sRGB 數值，未預乘）。
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(rgba.len(), (width * height * 4) as usize, "影格大小不符");
        Frame { width, height, rgba }
    }

    /// 雙線性取樣，回傳 0–1 的 RGB。
    pub fn sample_rgb(&self, u: f32, v: f32) -> [f32; 3] {
        let x = (u * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let y = (v * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let px = |x: u32, y: u32, c: usize| self.rgba[((y * self.width + x) * 4) as usize + c] as f32 / 255.0;
        let mut out = [0.0; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let top = px(x0, y0, c) * (1.0 - fx) + px(x1, y0, c) * fx;
            let bot = px(x0, y1, c) * (1.0 - fx) + px(x1, y1, c) * fx;
            *o = top * (1.0 - fy) + bot * fy;
        }
        out
    }
}

/// 單通道遮罩，0–1。
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

impl Mask {
    pub fn filled(width: u32, height: u32, v: f32) -> Self {
        Mask { width, height, data: vec![v; (width * height) as usize] }
    }

    pub fn get(&self, x: u32, y: u32) -> f32 {
        self.data[(y * self.width + x) as usize]
    }

    pub fn to_u8(&self) -> Vec<u8> {
        self.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect()
    }

    /// 以亮度當遮罩（Rec.709 係數）。
    pub fn from_luma(frame: &Frame, width: u32, height: u32) -> Self {
        let mut m = Mask::filled(width, height, 0.0);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = frame.sample_rgb((x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32);
                m.data[(y * width + x) as usize] = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            }
        }
        m
    }
}
