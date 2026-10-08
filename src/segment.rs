//! 人物分割：ONNX 模型（ort）＋兩個不靠 AI 的遮罩來源，以及背景執行緒。
//!
//! 支援的模型：
//! * Robust Video Matting（`rvm_mobilenetv3_fp32.onnx`，預設）：全身、時間穩定，輸入任意尺寸。
//! * 單一輸入的分割模型（NCHW 或 NHWC，固定尺寸，輸出人物機率），如 MediaPipe selfie 的 ONNX 轉檔。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::Result;

use crate::frame::{Frame, Mask};
use crate::matte;
use crate::params::MatteSource;

pub trait Segmenter: Send {
    fn name(&self) -> String;
    /// 產生 `w × h` 的人物遮罩。
    fn segment(&mut self, frame: &Frame, w: u32, h: u32) -> Result<Mask>;
    /// 換來源時清掉時間狀態（RVM 的遞迴狀態）。
    fn reset(&mut self) {}
}

pub const DEFAULT_MODEL_FILE: &str = "rvm_mobilenetv3_fp32.onnx";

/// 找模型檔：環境變數 → ./models → 執行檔旁的 models。
pub fn find_model() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TIME_ECHO_MODEL").map(PathBuf::from) {
        return Some(p);
    }
    let mut dirs = vec![PathBuf::from("models")];
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors().skip(1).take(4) {
            dirs.push(anc.join("models"));
        }
    }
    dirs.into_iter().map(|d| d.join(DEFAULT_MODEL_FILE)).find(|p| p.exists())
}

/// 找 ONNX Runtime 動態函式庫：ORT_DYLIB_PATH → 執行檔旁 → Homebrew／./lib。
/// Windows 一定要用絕對路徑：只給檔名會先載到 System32 裡系統附帶的舊版 onnxruntime.dll。
pub fn find_ort_library() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from) {
        return Some(p);
    }
    let file = if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else {
        "libonnxruntime.so"
    };
    let mut candidates = Vec::new();
    if let Some(d) = crate::source::exe_dir() {
        candidates.push(d.join(file));
        candidates.push(d.join("lib").join(file));
    }
    candidates.push(PathBuf::from("lib").join(file));
    if cfg!(target_os = "macos") {
        candidates.extend(["/opt/homebrew/lib", "/usr/local/lib"].map(|d| Path::new(d).join(file)));
    } else if cfg!(target_os = "linux") {
        candidates.extend(["/usr/local/lib", "/usr/lib"].map(|d| Path::new(d).join(file)));
    }
    candidates.into_iter().find(|p| p.exists()).and_then(|p| std::path::absolute(p).ok())
}

/// 分割模型的下載網址（Robust Video Matting 官方 release，約 15 MB）
pub const MODEL_URL: &str = "https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx";

/// 用系統內建的 curl（macOS、Windows 10 以後都有）下載模型到執行檔旁的 models/，
/// 那裡寫不進去就放 ./models。阻塞，請在背景執行緒呼叫。
pub fn download_model() -> Result<PathBuf> {
    use anyhow::{Context, bail};
    let dirs: Vec<PathBuf> = crate::source::exe_dir().map(|d| d.join("models")).into_iter().chain([PathBuf::from("models")]).collect();
    let dir = dirs.into_iter().find(|d| std::fs::create_dir_all(d).is_ok()).context("沒有可寫入的 models 資料夾")?;
    let dest = dir.join(DEFAULT_MODEL_FILE);
    let part = dir.join(format!("{DEFAULT_MODEL_FILE}.part"));
    let out = crate::source::command("curl")
        .args(["-L", "--fail", "--silent", "--show-error", "-o"])
        .arg(&part)
        .arg(MODEL_URL)
        .output()
        .context("找不到 curl，請用瀏覽器下載模型")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&part);
        bail!("下載失敗：{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    std::fs::rename(&part, &dest).context("無法存檔")?;
    Ok(dest)
}

pub fn ai_available() -> bool {
    cfg!(feature = "seg")
}

/// 開啟 AI 分割器。
pub fn open_ai(model: &Path) -> Result<Box<dyn Segmenter>> {
    #[cfg(feature = "seg")]
    {
        Ok(Box::new(onnx::OnnxSegmenter::open(model)?))
    }
    #[cfg(not(feature = "seg"))]
    {
        let _ = model;
        anyhow::bail!("這個版本編譯時沒有開 seg 功能，無法用 AI 分割")
    }
}

/// 以亮度當遮罩：黑底素材用，配合 Clip black／white 調門檻。
pub struct LumaSegmenter;

impl Segmenter for LumaSegmenter {
    fn name(&self) -> String {
        "亮度遮罩".into()
    }
    fn segment(&mut self, frame: &Frame, w: u32, h: u32) -> Result<Mask> {
        Ok(Mask::from_luma(frame, w, h))
    }
}

/// 把 RGB 雙線性縮放成 `w × h` 的 0–1 平面陣列（CHW 或 HWC）。
pub fn frame_to_tensor(frame: &Frame, w: u32, h: u32, chw: bool) -> Vec<f32> {
    let n = (w * h) as usize;
    let mut out = vec![0.0f32; n * 3];
    for y in 0..h {
        for x in 0..w {
            let rgb = frame.sample_rgb((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            let i = (y * w + x) as usize;
            for c in 0..3 {
                if chw {
                    out[c * n + i] = rgb[c];
                } else {
                    out[i * 3 + c] = rgb[c];
                }
            }
        }
    }
    out
}

/// 雙線性縮放遮罩。
pub fn resize_mask(src: &Mask, w: u32, h: u32) -> Mask {
    if src.width == w && src.height == h {
        return src.clone();
    }
    let mut m = Mask::filled(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let fx = ((x as f32 + 0.5) / w as f32 * src.width as f32 - 0.5).clamp(0.0, (src.width - 1) as f32);
            let fy = ((y as f32 + 0.5) / h as f32 * src.height as f32 - 0.5).clamp(0.0, (src.height - 1) as f32);
            let (x0, y0) = (fx as u32, fy as u32);
            let (x1, y1) = ((x0 + 1).min(src.width - 1), (y0 + 1).min(src.height - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let top = src.get(x0, y0) * (1.0 - tx) + src.get(x1, y0) * tx;
            let bot = src.get(x0, y1) * (1.0 - tx) + src.get(x1, y1) * tx;
            m.data[(y * w + x) as usize] = top * (1.0 - ty) + bot * ty;
        }
    }
    m
}

#[cfg(feature = "seg")]
mod onnx {
    use super::*;
    use anyhow::{Context, anyhow};
    use ort::session::Session;
    use ort::value::Tensor;
    use std::sync::OnceLock;

    static ORT_INIT: OnceLock<Result<(), String>> = OnceLock::new();

    fn init_runtime() -> Result<()> {
        ORT_INIT
            .get_or_init(|| {
                let lib = find_ort_library().ok_or_else(|| {
                    if cfg!(target_os = "windows") {
                        "找不到 ONNX Runtime：onnxruntime.dll 要放在 time-echo.exe 旁".to_string()
                    } else {
                        "找不到 ONNX Runtime 函式庫：請 brew install onnxruntime，或設定 ORT_DYLIB_PATH".to_string()
                    }
                })?;
                ort::init_from(&lib).map_err(|e| format!("載入 {} 失敗：{e}", lib.display()))?.with_name("time-echo").commit();
                Ok(())
            })
            .clone()
            .map_err(|e| anyhow!(e))
    }

    enum Kind {
        /// Robust Video Matting：src + r1i..r4i + downsample_ratio
        Rvm { state: [(Vec<i64>, Vec<f32>); 4] },
        /// 單一輸入：固定尺寸
        Simple { input: String, w: u32, h: u32, chw: bool },
    }

    pub struct OnnxSegmenter {
        session: Session,
        kind: Kind,
        file: String,
    }

    fn zero_state() -> [(Vec<i64>, Vec<f32>); 4] {
        std::array::from_fn(|_| (vec![1, 1, 1, 1], vec![0.0]))
    }

    impl OnnxSegmenter {
        pub fn open(path: &Path) -> Result<Self> {
            init_runtime()?;
            let threads = thread::available_parallelism().map(|n| n.get().min(4)).unwrap_or(2);
            let session = Session::builder()
                .map_err(|e| anyhow!("{e}"))?
                .with_intra_threads(threads)
                .map_err(|e| anyhow!("{e}"))?
                .commit_from_file(path)
                .with_context(|| format!("讀不了模型 {}", path.display()))?;
            let names: Vec<String> = session.inputs().iter().map(|i| i.name().to_string()).collect();
            let kind = if names.iter().any(|n| n == "r1i") && names.iter().any(|n| n == "downsample_ratio") {
                Kind::Rvm { state: zero_state() }
            } else if names.len() == 1 {
                let shape: Vec<i64> = session.inputs()[0]
                    .dtype()
                    .tensor_shape()
                    .map(|s| s.iter().copied().collect())
                    .ok_or_else(|| anyhow!("模型輸入不是張量"))?;
                if shape.len() != 4 {
                    anyhow::bail!("模型輸入應為 4 維，實際 {shape:?}");
                }
                let chw = shape[1] == 3;
                let (h, w) = if chw { (shape[2], shape[3]) } else { (shape[1], shape[2]) };
                let (w, h) = (if w > 0 { w as u32 } else { 256 }, if h > 0 { h as u32 } else { 256 });
                Kind::Simple { input: names[0].clone(), w, h, chw }
            } else {
                anyhow::bail!("不認得這個模型的輸入：{names:?}");
            };
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Ok(OnnxSegmenter { session, kind, file })
        }
    }

    impl Segmenter for OnnxSegmenter {
        fn name(&self) -> String {
            self.file.clone()
        }

        fn reset(&mut self) {
            if let Kind::Rvm { state } = &mut self.kind {
                *state = zero_state();
            }
        }

        fn segment(&mut self, frame: &Frame, w: u32, h: u32) -> Result<Mask> {
            let e = |e: ort::Error| anyhow!("{e}");
            match &mut self.kind {
                Kind::Rvm { state } => {
                    // RVM 建議：HD 0.25、4K 0.125 → 以長邊 480 px 為準
                    let ratio = (480.0 / w.max(h) as f32).min(1.0);
                    let src = Tensor::from_array((vec![1i64, 3, h as i64, w as i64], frame_to_tensor(frame, w, h, true))).map_err(e)?;
                    let r: Vec<Tensor<f32>> = state
                        .iter()
                        .map(|(s, d)| Tensor::from_array((s.clone(), d.clone())))
                        .collect::<Result<_, _>>()
                        .map_err(e)?;
                    let [r1, r2, r3, r4]: [Tensor<f32>; 4] = r.try_into().map_err(|_| anyhow!("內部錯誤"))?;
                    let ds = Tensor::from_array((vec![1i64], vec![ratio])).map_err(e)?;
                    let outputs = self
                        .session
                        .run(ort::inputs!["src" => src, "r1i" => r1, "r2i" => r2, "r3i" => r3, "r4i" => r4, "downsample_ratio" => ds])
                        .map_err(e)?;
                    let (_, pha) = outputs["pha"].try_extract_tensor::<f32>().map_err(e)?;
                    let mask = Mask { width: w, height: h, data: pha.to_vec() };
                    for (i, name) in ["r1o", "r2o", "r3o", "r4o"].iter().enumerate() {
                        let (shape, data) = outputs[*name].try_extract_tensor::<f32>().map_err(e)?;
                        state[i] = (shape.iter().copied().collect(), data.to_vec());
                    }
                    Ok(mask)
                }
                Kind::Simple { input, w: mw, h: mh, chw } => {
                    let (mw, mh, chw) = (*mw, *mh, *chw);
                    let shape = if chw { vec![1i64, 3, mh as i64, mw as i64] } else { vec![1i64, mh as i64, mw as i64, 3] };
                    let t = Tensor::from_array((shape, frame_to_tensor(frame, mw, mh, chw))).map_err(e)?;
                    let outputs = self.session.run(ort::inputs![input.as_str() => t]).map_err(e)?;
                    let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(e)?;
                    let dims: Vec<i64> = shape.iter().copied().collect();
                    // 輸出：[1,C,H,W] 或 [1,H,W,C] 或 [1,H,W]
                    let (oh, ow, c, out_chw) = match dims.as_slice() {
                        [1, c, h, w] if *c <= 4 && *h > 4 => (*h as usize, *w as usize, *c as usize, true),
                        [1, h, w, c] => (*h as usize, *w as usize, *c as usize, false),
                        [1, h, w] => (*h as usize, *w as usize, 1, true),
                        _ => anyhow::bail!("不認得模型輸出形狀 {dims:?}"),
                    };
                    let n = oh * ow;
                    let needs_sigmoid = data.iter().any(|v| *v < 0.0 || *v > 1.0);
                    let mut m = Mask::filled(ow as u32, oh as u32, 0.0);
                    for i in 0..n {
                        let at = |ch: usize| if out_chw { data[ch * n + i] } else { data[i * c + ch] };
                        let v = match c {
                            1 => at(0),
                            2 => at(1),
                            _ => 1.0 - at(0), // 多類別：1 − 背景
                        };
                        m.data[i] = if needs_sigmoid { 1.0 / (1.0 + (-v).exp()) } else { v };
                    }
                    Ok(resize_mask(&m, w, h))
                }
            }
        }
    }
}

// ───────────── 背景執行緒 ─────────────

pub struct SegJob {
    pub frame: Arc<Frame>,
    pub width: u32,
    pub height: u32,
    pub source: MatteSource,
    /// MOV / video has alpha：優先用影片 alpha
    pub video_alpha: bool,
    pub screen: [f32; 3],
    /// 0–2
    pub screen_gain: f32,
    /// 0–1
    pub screen_balance: f32,
    pub shrink_grow: f32,
    pub softness: f32,
    /// 遮罩寬 ÷ 緩衝寬（px 換算）
    pub px_scale: f32,
}

impl SegJob {
    pub fn from_params(frame: Arc<Frame>, width: u32, height: u32, p: &crate::params::Params) -> Self {
        SegJob {
            frame,
            width,
            height,
            source: p.matte_source,
            video_alpha: p.video_alpha,
            screen: p.screen_colour.to_f32(),
            screen_gain: p.screen_gain / 100.0,
            screen_balance: p.screen_balance / 100.0,
            shrink_grow: p.shrink_grow,
            softness: p.edge_softness,
            px_scale: 1.0,
        }
    }
}

/// 依遮罩來源算出整理好的遮罩。`ai` 為 None（模型未載入）時 AI 部分當成全畫面。
pub fn build_mask(ai: Option<&mut (dyn Segmenter + 'static)>, job: &SegJob) -> Result<Mask> {
    let (w, h) = (job.width, job.height);
    let key = || crate::key::key_mask(&job.frame, w, h, job.screen, job.screen_gain, job.screen_balance);
    let ai_mask = |ai: Option<&mut (dyn Segmenter + 'static)>| match ai {
        Some(s) => s.segment(&job.frame, w, h),
        None => Ok(Mask::filled(w, h, 1.0)),
    };
    let mut mask = if job.video_alpha {
        crate::key::alpha_mask(&job.frame, w, h)
    } else {
        match job.source {
            MatteSource::Ai => ai_mask(ai)?,
            MatteSource::Key => key(),
            MatteSource::AiKey => {
                let mut m = ai_mask(ai)?;
                crate::key::multiply(&mut m, &key());
                m
            }
            MatteSource::Luma => Mask::from_luma(&job.frame, w, h),
            MatteSource::Full => Mask::filled(w, h, 1.0),
        }
    };
    matte::refine(&mut mask, job.shrink_grow, job.softness, job.px_scale);
    Ok(mask)
}

#[derive(Default)]
struct Shared {
    job: Option<SegJob>,
    result: Option<(u64, Mask)>,
    load: Option<PathBuf>,
    reset: bool,
    quit: bool,
    status: String,
    model_name: Option<String>,
    ai_fps: f32,
}

/// 分割在背景跑，「最新的影格優先」：還沒處理的舊工作會被新工作蓋掉。
pub struct SegWorker {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    last_seen: u64,
}

impl SegWorker {
    pub fn spawn() -> Self {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let s2 = shared.clone();
        thread::spawn(move || worker_loop(s2));
        SegWorker { shared, last_seen: 0 }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        let (m, cv) = &*self.shared;
        let r = f(&mut m.lock().unwrap());
        cv.notify_all();
        r
    }

    pub fn load_model(&self, path: PathBuf) {
        self.with(|s| {
            s.status = format!("載入模型 {}…", path.display());
            s.load = Some(path)
        });
    }

    pub fn submit(&self, job: SegJob) {
        self.with(|s| s.job = Some(job));
    }

    pub fn reset(&self) {
        self.with(|s| s.reset = true);
    }

    /// 有新的遮罩才回傳。
    pub fn take_new(&mut self) -> Option<Mask> {
        let last = self.last_seen;
        let r = self.with(|s| s.result.as_ref().filter(|(seq, _)| *seq > last).map(|(seq, m)| (*seq, m.clone())));
        r.map(|(seq, m)| {
            self.last_seen = seq;
            m
        })
    }

    pub fn status(&self) -> (String, Option<String>, f32) {
        self.with(|s| (s.status.clone(), s.model_name.clone(), s.ai_fps))
    }
}

impl Drop for SegWorker {
    fn drop(&mut self) {
        self.with(|s| s.quit = true);
    }
}

fn worker_loop(shared: Arc<(Mutex<Shared>, Condvar)>) {
    let (m, cv) = &*shared;
    let mut ai: Option<Box<dyn Segmenter>> = None;
    let mut seq = 0u64;
    let mut last_done: Option<Instant> = None;
    let mut fps = 0.0f32;
    loop {
        let (job, load, reset) = {
            let mut s = m.lock().unwrap();
            while s.job.is_none() && s.load.is_none() && !s.reset && !s.quit {
                s = cv.wait(s).unwrap();
            }
            if s.quit {
                return;
            }
            let r = (s.job.take(), s.load.take(), std::mem::take(&mut s.reset));
            r
        };
        if let Some(path) = load {
            let (status, name) = match open_ai(&path) {
                Ok(seg) => {
                    let name = seg.name();
                    ai = Some(seg);
                    (format!("AI 人物分割就緒：{name}"), Some(name))
                }
                Err(e) => {
                    ai = None;
                    (format!("AI 分割無法使用：{e:#}"), None)
                }
            };
            let mut s = m.lock().unwrap();
            s.status = status;
            s.model_name = name;
        }
        if reset {
            if let Some(a) = ai.as_mut() {
                a.reset();
            }
        }
        let Some(job) = job else { continue };
        let needs_ai = !job.video_alpha && matches!(job.source, MatteSource::Ai | MatteSource::AiKey);
        let mask = build_mask(if needs_ai { ai.as_deref_mut() } else { None }, &job);
        match mask {
            Ok(mask) => {
                let now = Instant::now();
                if let Some(t) = last_done {
                    let inst = 1.0 / now.duration_since(t).as_secs_f32().max(1e-3);
                    fps = if fps == 0.0 { inst } else { fps * 0.9 + inst * 0.1 };
                }
                last_done = Some(now);
                seq += 1;
                let mut s = m.lock().unwrap();
                s.result = Some((seq, mask));
                s.ai_fps = fps;
            }
            Err(e) => {
                m.lock().unwrap().status = format!("分割錯誤：{e:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tensor_layouts() {
        let f = Frame::new(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
        let chw = frame_to_tensor(&f, 2, 1, true);
        assert_eq!(chw, vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let hwc = frame_to_tensor(&f, 2, 1, false);
        assert_eq!(hwc, vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn worker_produces_luma_mask() {
        let mut w = SegWorker::spawn();
        let frame = Arc::new(Frame::new(2, 2, vec![255; 16]));
        let p = crate::params::Params { matte_source: MatteSource::Luma, edge_softness: 0.0, ..Default::default() };
        w.submit(SegJob::from_params(frame, 4, 4, &p));
        let t = Instant::now();
        loop {
            if let Some(m) = w.take_new() {
                assert_eq!((m.width, m.height), (4, 4));
                assert!(m.data.iter().all(|v| (*v - 1.0).abs() < 1e-4));
                break;
            }
            assert!(t.elapsed().as_secs() < 5, "逾時");
            thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
