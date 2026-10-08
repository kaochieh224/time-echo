//! 離線算圖：不開視窗，把影片檔跑過整條管線輸出成 MP4。
//! 用來在沒有攝影機或螢幕的環境驗證核心殘影管線，也可以拿來批次出片。

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use eframe::wgpu;

use crate::engine::Engine;
use crate::params::{MatteSource, Params};
use crate::record::Recorder;
use crate::render::Renderer;
use crate::segment::{self, LumaSegmenter, Segmenter};
use crate::source::VideoSource;

pub struct RenderOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub params: Params,
    pub model: Option<PathBuf>,
    pub max_frames: Option<u64>,
    /// 影片預設不鏡像；`--mirror` 才開
    pub mirror: bool,
}

pub struct RenderStats {
    pub frames: u64,
    pub captured: u64,
    pub seconds: f32,
    pub adapter: String,
    pub segmenter: String,
    pub last_visible_echoes: usize,
}

/// 建立沒有視窗的 GPU 裝置（macOS 走 Metal；Linux 容器可用 lavapipe 軟體繪圖）。
pub fn create_device() -> Result<(wgpu::Device, wgpu::Queue, String)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .map_err(|e| anyhow!("找不到可用的 GPU：{e}"))?;
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("time-echo headless"),
        ..Default::default()
    }))
    .context("建立 GPU 裝置失敗")?;
    Ok((device, queue, format!("{}（{:?}）", info.name, info.backend)))
}

pub fn run(opts: RenderOptions, mut progress: impl FnMut(u64)) -> Result<RenderStats> {
    let started = Instant::now();
    let (device, queue, adapter) = create_device()?;
    let mut p = opts.params;
    p.mirror = opts.mirror; // 影片來源預設不鏡像（規格 3.1）
    let mut source = VideoSource::open(&opts.input, false)?;
    let fps = source.info.fps;
    let mut engine = Engine::new(Renderer::new(device, queue), &p);

    let mut seg: Option<Box<dyn Segmenter>> = match p.matte_source {
        MatteSource::Ai => {
            let model = opts.model.or_else(segment::find_model).ok_or_else(|| {
                anyhow!("找不到分割模型：請放在 models/{}，或用 --model 指定", segment::DEFAULT_MODEL_FILE)
            })?;
            Some(segment::open_ai(&model)?)
        }
        MatteSource::Luma => Some(Box::new(LumaSegmenter)),
        MatteSource::Full => None,
    };
    let segmenter = seg.as_ref().map(|s| s.name()).unwrap_or_else(|| "全畫面".into());

    let mut recorder: Option<Recorder> = None;
    let mut frames = 0u64;
    while let Some(frame) = source.next_blocking() {
        if opts.max_frames.is_some_and(|m| frames >= m) {
            break;
        }
        let (bw, bh) = engine.buffer_dims(frame.width, frame.height);
        let mask = match seg.as_mut() {
            Some(s) => {
                let mut m = s.segment(&frame, bw, bh)?;
                crate::matte::refine(&mut m, p.shrink_grow, p.edge_softness, 1.0);
                Some(m)
            }
            None => Some(crate::frame::Mask::filled(1, 1, 1.0)),
        };
        engine.push_frame(&p, &frame, mask.as_ref(), 1.0 / fps);
        let (w, h) = engine.render(&p, frames as f64 / fps as f64, false);
        let out = engine.renderer.read_output().context("讀回輸出失敗")?;
        if recorder.is_none() {
            recorder = Some(Recorder::start(&opts.output, w, h, fps)?);
        }
        recorder.as_mut().unwrap().write(&out)?;
        frames += 1;
        progress(frames);
    }
    let rec = recorder.ok_or_else(|| anyhow!("影片沒有任何影格"))?;
    rec.finish()?;
    Ok(RenderStats {
        frames,
        captured: engine.captured_total,
        seconds: started.elapsed().as_secs_f32(),
        adapter,
        segmenter,
        last_visible_echoes: engine.last_plan.instances.len(),
    })
}
