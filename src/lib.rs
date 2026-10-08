//! 時間分身特效控制器（TIME & MEMORY 風格）原型。
//!
//! 管線：來源 → 人物分割 → 擷取進環形記憶緩衝 → 依「分身數 × 時間間隔」取歷史影格 →
//! 依間距、尺寸、軸心排開疊合 → 表面／調色 → Bloom → 輸出。

pub mod app;
pub mod beat;
pub mod curve;
pub mod engine;
pub mod frame;
pub mod headless;
pub mod matte;
pub mod memory;
pub mod params;
pub mod record;
pub mod render;
pub mod segment;
pub mod source;
