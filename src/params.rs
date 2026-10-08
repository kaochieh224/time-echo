//! 參數表：規格第 3 節的單一來源。
//!
//! 數值一律以「面板單位」存放（%、秒、px），和規格表與預設組 JSON 一致；
//! 換算成 0–1 的工作在 `render` 與 `engine` 內做。

use serde::{Deserialize, Serialize};

/// 以 `#RRGGBB` 存進 JSON 的顏色。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    pub const fn hex(v: u32) -> Self {
        Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8])
    }
    pub fn to_f32(self) -> [f32; 3] {
        self.0.map(|c| c as f32 / 255.0)
    }
    pub fn to_hex_string(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0[0], self.0[1], self.0[2])
    }
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        u32::from_str_radix(s, 16).ok().map(Rgb::hex)
    }
}

impl Serialize for Rgb {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex_string())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("顏色格式錯誤：{s}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatteSource {
    /// AI 人物分割（ONNX 模型）
    Ai,
    /// 05 Keylight 色鍵
    Key,
    /// AI 遮罩 × 色鍵遮罩
    AiKey,
    /// 以亮度當遮罩：適合黑底舞蹈影片，配合 Clip black／white 調門檻
    Luma,
    /// 不去背，整張畫面當成分身
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// 最新分身在 Axis X，舊的往一側排
    Procession,
    /// 整排的中心在 Axis X
    Centered,
    /// 最新在 Axis X，舊的左右交替展開
    Symmetric,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// Textured video echoes
    Textured,
    /// Delayed solid：實心剪影
    Solid,
    /// Contour：只留輪廓線
    Contour,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColourMode {
    Original,
    White,
    Mono,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BgMode {
    Solid,
    Source,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    // ── 全域 ──
    /// 輸出長邊 px：960／1280／1920
    pub output_res: u32,

    // ── 01 來源 ──
    pub mirror: bool,
    pub matte_source: MatteSource,
    /// MOV / video has alpha：用影片自帶的透明通道，略過 AI 與色鍵
    pub video_alpha: bool,
    pub monitors: bool,
    /// 緩衝長邊 px：480／640／960
    pub buffer_res: u32,
    /// 記憶取樣率 fps，10–30
    pub capture_rate: f32,
    /// 記憶長度 s，2–16
    pub memory_length: f32,

    // ── 02 時間分身 ──
    pub echo_count: u32,
    pub interval: f32,
    pub spacing: f32,
    pub figure_size: f32,
    pub axis_x: f32,
    pub layout: Layout,
    pub echo_fade: f32,

    // ── 03 表面與光 ──
    pub surface: Surface,
    pub disintegrate: f32,
    pub bloom: f32,
    pub colour_mode: ColourMode,
    pub custom_colour: Rgb,
    pub bg_mode: BgMode,
    pub bg_colour: Rgb,
    pub ground_shadows: bool,
    pub shadow_opacity: f32,

    // ── 04 調色 ──
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub curve_shadows: f32,
    pub curve_mids: f32,
    pub curve_highs: f32,
    pub duotone: bool,
    pub duotone_dark: Rgb,
    pub duotone_light: Rgb,
    /// Selective colour：只調整接近取樣色相的部分
    pub selective: bool,
    pub selective_colour: Rgb,
    /// 色相容差（度）
    pub selective_tolerance: f32,
    /// 色相位移（度）
    pub selective_hue: f32,
    pub selective_sat: f32,
    pub selective_light: f32,

    // ── 05 Keylight ──
    pub screen_colour: Rgb,
    pub screen_gain: f32,
    pub screen_balance: f32,
    pub despill: f32,
    pub clip_black: f32,
    pub clip_white: f32,
    pub shrink_grow: f32,
    pub edge_softness: f32,

    // ── 節拍 ──
    pub bpm: f32,
    pub beat_sync: bool,
    /// 間隔拍數：0.125／0.25／0.5／1／2
    pub beat_division: f32,
    pub beat_pulse: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            output_res: 1280,
            mirror: true,
            matte_source: MatteSource::Ai,
            video_alpha: false,
            monitors: true,
            buffer_res: 640,
            capture_rate: 15.0,
            memory_length: 8.0,
            echo_count: 12,
            interval: 0.30,
            spacing: 100.0,
            figure_size: 100.0,
            axis_x: 0.0,
            layout: Layout::Procession,
            echo_fade: 0.0,
            surface: Surface::Textured,
            disintegrate: 0.0,
            bloom: 0.0,
            colour_mode: ColourMode::Original,
            custom_colour: Rgb::hex(0xFFFFFF),
            bg_mode: BgMode::Solid,
            bg_colour: Rgb::hex(0x000000),
            ground_shadows: false,
            shadow_opacity: 50.0,
            brightness: 0.0,
            contrast: 100.0,
            saturation: 100.0,
            curve_shadows: 0.0,
            curve_mids: 0.0,
            curve_highs: 0.0,
            duotone: false,
            duotone_dark: Rgb::hex(0x1A1AFF),
            duotone_light: Rgb::hex(0xFF000C),
            selective: false,
            selective_colour: Rgb::hex(0xFF000C),
            selective_tolerance: 30.0,
            selective_hue: 0.0,
            selective_sat: 0.0,
            selective_light: 0.0,
            screen_colour: Rgb::hex(0x00B140),
            screen_gain: 100.0,
            screen_balance: 50.0,
            despill: 0.0,
            clip_black: 0.0,
            clip_white: 100.0,
            shrink_grow: 0.0,
            edge_softness: 1.0,
            bpm: 120.0,
            beat_sync: false,
            beat_division: 0.5,
            beat_pulse: 0.0,
        }
    }
}

impl Params {
    /// 規格第 4 節「示範（TIME & MEMORY）」預設組。
    pub fn demo() -> Self {
        Params {
            mirror: false,
            echo_count: 22,
            interval: 0.30,
            spacing: 241.0,
            figure_size: 170.0,
            axis_x: 72.0,
            disintegrate: 100.0,
            bloom: 25.0,
            colour_mode: ColourMode::Original,
            bg_colour: Rgb::hex(0xFF000C),
            brightness: -30.0,
            contrast: 160.0,
            saturation: 200.0,
            curve_shadows: 43.0,
            curve_mids: 0.0,
            curve_highs: -25.0,
            screen_colour: Rgb::hex(0x000000),
            screen_gain: 107.0,
            screen_balance: 91.0,
            despill: 42.0,
            clip_black: 53.0,
            clip_white: 65.0,
            shrink_grow: -1.0,
            edge_softness: 2.5,
            bpm: 126.0,
            ..Params::default()
        }
    }

    /// 把超出範圍的值夾回規格範圍（載入外部 JSON 後呼叫）。
    pub fn sanitize(&mut self) {
        self.output_res = nearest(self.output_res, &[960, 1280, 1920]);
        self.buffer_res = nearest(self.buffer_res, &[480, 640, 960]);
        self.capture_rate = self.capture_rate.clamp(10.0, 30.0);
        self.memory_length = self.memory_length.clamp(2.0, 16.0);
        self.echo_count = self.echo_count.clamp(1, 48);
        self.interval = self.interval.clamp(0.05, 2.0);
        self.spacing = self.spacing.clamp(-300.0, 300.0);
        self.figure_size = self.figure_size.clamp(10.0, 300.0);
        self.axis_x = self.axis_x.clamp(-100.0, 100.0);
        self.echo_fade = self.echo_fade.clamp(0.0, 100.0);
        self.disintegrate = self.disintegrate.clamp(0.0, 100.0);
        self.bloom = self.bloom.clamp(0.0, 100.0);
        self.brightness = self.brightness.clamp(-100.0, 100.0);
        self.contrast = self.contrast.clamp(0.0, 200.0);
        self.saturation = self.saturation.clamp(0.0, 200.0);
        self.curve_shadows = self.curve_shadows.clamp(-100.0, 100.0);
        self.curve_mids = self.curve_mids.clamp(-100.0, 100.0);
        self.curve_highs = self.curve_highs.clamp(-100.0, 100.0);
        self.clip_black = self.clip_black.clamp(0.0, 99.0);
        self.clip_white = self.clip_white.clamp(self.clip_black + 1.0, 100.0);
        self.shrink_grow = self.shrink_grow.clamp(-10.0, 10.0).round();
        self.edge_softness = self.edge_softness.clamp(0.0, 10.0);
        self.bpm = self.bpm.clamp(60.0, 200.0);
        self.beat_division = [0.125, 0.25, 0.5, 1.0, 2.0]
            .into_iter()
            .min_by(|a, b| (a - self.beat_division).abs().total_cmp(&(b - self.beat_division).abs()))
            .unwrap();
        self.beat_pulse = self.beat_pulse.clamp(0.0, 100.0);
        self.shadow_opacity = self.shadow_opacity.clamp(0.0, 100.0);
        self.selective_tolerance = self.selective_tolerance.clamp(5.0, 90.0);
        self.selective_hue = self.selective_hue.clamp(-180.0, 180.0);
        self.selective_sat = self.selective_sat.clamp(-100.0, 100.0);
        self.selective_light = self.selective_light.clamp(-100.0, 100.0);
        self.screen_gain = self.screen_gain.clamp(0.0, 200.0);
        self.screen_balance = self.screen_balance.clamp(0.0, 100.0);
        self.despill = self.despill.clamp(0.0, 100.0);
    }

    /// 實際生效的 Temporal interval：開 Beat sync 時由 BPM 換算。
    pub fn effective_interval(&self) -> f32 {
        if self.beat_sync {
            60.0 / self.bpm * self.beat_division
        } else {
            self.interval
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Params 一定能序列化")
    }

    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        let mut p: Params = serde_json::from_str(s)?;
        p.sanitize();
        Ok(p)
    }
}

fn nearest(v: u32, options: &[u32]) -> u32 {
    *options.iter().min_by_key(|o| o.abs_diff(v)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_keeps_values() {
        let p = Params::demo();
        let back = Params::from_json(&p.to_json()).unwrap();
        assert_eq!(p, back);
        assert!(p.to_json().contains("\"bg_colour\": \"#FF000C\""));
    }

    #[test]
    fn partial_json_fills_defaults_and_clamps() {
        let p = Params::from_json(r#"{"echo_count": 999, "clip_black": 80, "clip_white": 20}"#).unwrap();
        assert_eq!(p.echo_count, 48);
        assert_eq!(p.clip_black, 80.0);
        assert_eq!(p.clip_white, 81.0);
        assert_eq!(p.spacing, Params::default().spacing);
    }

    #[test]
    fn beat_sync_interval() {
        let p = Params { beat_sync: true, bpm: 126.0, beat_division: 0.5, ..Params::default() };
        assert!((p.effective_interval() - 0.238).abs() < 0.001);
    }
}
