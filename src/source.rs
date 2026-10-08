//! 來源：影片檔（ffmpeg 解碼）、攝影機（nokhwa，或 ffmpeg 的 AVFoundation／DirectShow 輸入）。

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread;

use anyhow::{Context, Result, anyhow, bail};

use crate::frame::Frame;

/// 影片解碼後的長邊上限：再大也只是多花上傳時間，緩衝最多 960 px。
pub const MAX_SOURCE_EDGE: u32 = 1280;

pub trait Source: Send {
    fn label(&self) -> String;
    /// 非阻塞：有新影格就回傳（影格, 與上一張的來源時間差 s）。
    fn poll(&mut self, now: f64) -> Option<(Frame, f32)>;
    fn set_paused(&mut self, paused: bool);
    fn paused(&self) -> bool;
    /// 攝影機為 true（換來源時 Mirror 預設開）
    fn is_live(&self) -> bool;
    /// 讀取執行緒回報的錯誤
    fn error(&self) -> Option<String> {
        None
    }
}

pub fn ffmpeg_bin() -> PathBuf {
    std::env::var_os("TIME_ECHO_FFMPEG").map(PathBuf::from).unwrap_or_else(|| find_tool("ffmpeg"))
}

fn ffprobe_bin() -> PathBuf {
    find_tool("ffprobe")
}

/// 缺 ffmpeg 時的安裝提示
pub const FFMPEG_HINT: &str = if cfg!(target_os = "windows") {
    "找不到 ffmpeg：ffmpeg.exe 與 ffprobe.exe 要放在 time-echo.exe 旁的 ffmpeg 資料夾"
} else {
    "找不到 ffmpeg（請先安裝：brew install ffmpeg）"
};

/// 執行檔所在資料夾（Windows 版把 ffmpeg、ONNX Runtime、模型放在這裡）
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// 先找執行檔旁（Windows 打包版），再找 Homebrew 的位置：從 Finder 開啟的 App 拿不到 shell 的 PATH。
fn find_tool(name: &str) -> PathBuf {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let mut dirs = Vec::new();
    if let Some(d) = exe_dir() {
        dirs.push(d.join("ffmpeg"));
        dirs.push(d);
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    dirs.into_iter().map(|d| d.join(&file)).find(|p| p.exists()).unwrap_or_else(|| PathBuf::from(file))
}

/// 子程序。Windows 的視窗程式呼叫 ffmpeg 時預設會多跳一個黑色主控台視窗，這裡關掉。
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

#[derive(Clone, Debug)]
pub struct VideoInfo {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub frames: Option<u64>,
}

/// 用 ffprobe 讀尺寸（已考慮旋轉）、影格率與總張數。
pub fn probe(path: &Path) -> Result<VideoInfo> {
    let out = command(ffprobe_bin())
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .arg("stream=codec_name,width,height,avg_frame_rate,r_frame_rate,nb_frames:stream_side_data=rotation:stream_tags=rotate")
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(path)
        .output()
        .context(FFMPEG_HINT)?;
    if !out.status.success() {
        bail!("ffprobe 讀不了這個檔案：{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let get = |key: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::trim));
    let w: u32 = get("width").and_then(|v| v.parse().ok()).ok_or_else(|| anyhow!("讀不到影片寬度"))?;
    let h: u32 = get("height").and_then(|v| v.parse().ok()).ok_or_else(|| anyhow!("讀不到影片高度"))?;
    let rate = |s: &str| -> Option<f32> {
        let (a, b) = s.split_once('/')?;
        let (a, b): (f32, f32) = (a.parse().ok()?, b.parse().ok()?);
        (b > 0.0 && a > 0.0).then_some(a / b)
    };
    let fps = get("avg_frame_rate").and_then(rate).or_else(|| get("r_frame_rate").and_then(rate)).unwrap_or(30.0);
    let rotation: i32 = get("rotation").or_else(|| get("TAG:rotate")).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) as i32;
    let (w, h) = if rotation.rem_euclid(180) == 90 { (h, w) } else { (w, h) };
    let frames = get("nb_frames").and_then(|v| v.parse().ok());
    let codec = get("codec_name").unwrap_or_default().to_string();
    Ok(VideoInfo { codec, width: w, height: h, fps, frames })
}

/// 解碼後的尺寸：長邊不超過 `max_edge`，寬高為偶數。
pub fn decoded_size(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    let s = (max_edge as f32 / w.max(h) as f32).min(1.0);
    let even = |v: u32| ((v as f32 * s / 2.0).round() as u32).max(1) * 2;
    (even(w), even(h))
}

/// ffmpeg 子程序輸出 rawvideo RGBA，讀取執行緒把影格送進有界通道。
struct FfmpegPipe {
    child: Child,
    rx: Receiver<Frame>,
    err_rx: Receiver<String>,
    last_error: Option<String>,
    width: u32,
    height: u32,
}

impl FfmpegPipe {
    fn spawn(input_args: &[String], width: u32, height: u32, queue: usize) -> Result<Self> {
        let mut child = command(ffmpeg_bin())
            .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .args(input_args)
            .args(["-vf", &format!("scale={width}:{height}:flags=bilinear"), "-pix_fmt", "rgba", "-f", "rawvideo", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context(FFMPEG_HINT)?;
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (tx, rx): (SyncSender<Frame>, _) = mpsc::sync_channel(queue);
        let (err_tx, err_rx) = mpsc::channel();
        let size = (width * height * 4) as usize;
        thread::spawn(move || {
            loop {
                let mut buf = vec![0u8; size];
                if stdout.read_exact(&mut buf).is_err() {
                    break;
                }
                if tx.send(Frame::new(width, height, buf)).is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            if !s.trim().is_empty() {
                let _ = err_tx.send(s.trim().to_string());
            }
        });
        Ok(FfmpegPipe { child, rx, err_rx, last_error: None, width, height })
    }

    fn poll_error(&mut self) {
        if let Ok(e) = self.err_rx.try_recv() {
            self.last_error = Some(e);
        }
    }
}

impl Drop for FfmpegPipe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 影片檔來源：循環播放、靜音，依影片影格率出影格。
pub struct VideoSource {
    pipe: FfmpegPipe,
    name: String,
    pub info: VideoInfo,
    paused: bool,
    next_due: Option<f64>,
}

impl VideoSource {
    /// `looping`：桌面程式用 true；離線算圖用 false，讀到檔尾就結束。
    pub fn open(path: &Path, looping: bool) -> Result<Self> {
        let info = probe(path)?;
        let (w, h) = decoded_size(info.width, info.height, MAX_SOURCE_EDGE);
        let mut args: Vec<String> = Vec::new();
        if looping {
            args.extend(["-stream_loop".into(), "-1".into()]);
        }
        // VP8／VP9 的 alpha 只有 libvpx 解碼器讀得到（MOV / video has alpha 用）
        match info.codec.as_str() {
            "vp9" => args.extend(["-c:v".into(), "libvpx-vp9".into()]),
            "vp8" => args.extend(["-c:v".into(), "libvpx".into()]),
            _ => {}
        }
        args.extend(["-an".into(), "-i".into(), path.to_string_lossy().into_owned()]);
        let pipe = FfmpegPipe::spawn(&args, w, h, 3)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(VideoSource { pipe, name, info: VideoInfo { width: w, height: h, ..info }, paused: false, next_due: None })
    }

    /// 離線算圖用：阻塞讀下一張，檔尾回傳 None。
    pub fn next_blocking(&mut self) -> Option<Frame> {
        self.pipe.rx.recv().ok()
    }
}

impl Source for VideoSource {
    fn label(&self) -> String {
        format!("影片 {}（{}×{}，{:.0} fps）", self.name, self.pipe.width, self.pipe.height, self.info.fps)
    }

    fn poll(&mut self, now: f64) -> Option<(Frame, f32)> {
        self.pipe.poll_error();
        if self.paused {
            return None;
        }
        let period = 1.0 / self.info.fps.max(1.0) as f64;
        let due = *self.next_due.get_or_insert(now);
        if now < due {
            return None;
        }
        match self.pipe.rx.try_recv() {
            Ok(f) => {
                let mut next = due + period;
                if now - next > 0.25 {
                    next = now; // 落後太多就追上，不要連續快轉
                }
                self.next_due = Some(next);
                Some((f, period as f32))
            }
            Err(_) => None,
        }
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.next_due = None;
    }

    fn paused(&self) -> bool {
        self.paused
    }

    fn is_live(&self) -> bool {
        false
    }

    fn error(&self) -> Option<String> {
        self.pipe.last_error.clone()
    }
}

/// 即時來源共用：只取最新一張，dt 用牆上時間。
struct LiveState {
    paused: bool,
    last_time: Option<f64>,
}

impl LiveState {
    fn take(&mut self, now: f64, newest: Option<Frame>) -> Option<(Frame, f32)> {
        let f = newest?;
        if self.paused {
            return None;
        }
        let dt = self.last_time.map(|t| (now - t) as f32).unwrap_or(1.0 / 30.0).clamp(0.0, 0.5);
        self.last_time = Some(now);
        Some((f, dt))
    }
}

/// 備援：用 ffmpeg 讀攝影機（macOS AVFoundation、Windows DirectShow、Linux V4L2）。
/// `device` 是裝置編號；Windows 也可以直接填 DirectShow 裝置名稱。
pub struct FfmpegCamera {
    pipe: FfmpegPipe,
    device: String,
    live: LiveState,
}

impl FfmpegCamera {
    pub fn open(device: &str) -> Result<Self> {
        let (w, h) = (1280, 720);
        let args: Vec<String> = if cfg!(target_os = "macos") {
            ["-f", "avfoundation", "-framerate", "30", "-video_size", "1280x720", "-i", &format!("{device}:none")]
                .map(String::from)
                .to_vec()
        } else if cfg!(target_os = "windows") {
            let name = if device.chars().all(|c| c.is_ascii_digit()) {
                let idx: usize = device.parse().unwrap_or(0);
                let names = dshow_video_devices();
                names.get(idx).cloned().ok_or_else(|| anyhow!("找不到第 {idx} 個攝影機（共 {} 個）", names.len()))?
            } else {
                device.to_string()
            };
            ["-f", "dshow", "-framerate", "30", "-video_size", "1280x720", "-i", &format!("video={name}")]
                .map(String::from)
                .to_vec()
        } else {
            ["-f", "v4l2", "-framerate", "30", "-video_size", "1280x720", "-i", &format!("/dev/video{device}")]
                .map(String::from)
                .to_vec()
        };
        let pipe = FfmpegPipe::spawn(&args, w, h, 2)?;
        Ok(FfmpegCamera { pipe, device: device.to_string(), live: LiveState { paused: false, last_time: None } })
    }
}

/// 列出 DirectShow 視訊裝置名稱（ffmpeg 把清單印在 stderr）。
fn dshow_video_devices() -> Vec<String> {
    let Ok(out) = command(ffmpeg_bin()).args(["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"]).output() else {
        return Vec::new();
    };
    parse_dshow_devices(&String::from_utf8_lossy(&out.stderr))
}

fn parse_dshow_devices(log: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_video = true; // 舊版 ffmpeg 用標題分段，新版每行標 (video)/(audio)
    for line in log.lines() {
        if line.contains("DirectShow video devices") {
            in_video = true;
        } else if line.contains("DirectShow audio devices") {
            in_video = false;
        }
        if line.contains("Alternative name") {
            continue;
        }
        let (Some(a), Some(b)) = (line.find('"'), line.rfind('"')) else { continue };
        if b <= a {
            continue;
        }
        let tagged_video = line.contains("(video)");
        let tagged_audio = line.contains("(audio)") || line.contains("(none)");
        if tagged_video || (in_video && !tagged_audio) {
            names.push(line[a + 1..b].to_string());
        }
    }
    names
}

impl Source for FfmpegCamera {
    fn label(&self) -> String {
        format!("攝影機 {}（ffmpeg）", self.device)
    }
    fn poll(&mut self, now: f64) -> Option<(Frame, f32)> {
        self.pipe.poll_error();
        let mut newest = None;
        loop {
            match self.pipe.rx.try_recv() {
                Ok(f) => newest = Some(f),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        self.live.take(now, newest)
    }
    fn set_paused(&mut self, paused: bool) {
        self.live.paused = paused;
        self.live.last_time = None;
    }
    fn paused(&self) -> bool {
        self.live.paused
    }
    fn is_live(&self) -> bool {
        true
    }
    fn error(&self) -> Option<String> {
        self.pipe.last_error.clone()
    }
}

#[cfg(feature = "camera")]
pub use nokhwa_cam::{NokhwaCamera, list_cameras};

#[cfg(feature = "camera")]
mod nokhwa_cam {
    use super::*;
    use nokhwa::pixel_format::RgbAFormat;
    use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn init_permission() {
        #[cfg(target_os = "macos")]
        {
            use std::sync::Once;
            static INIT: Once = Once::new();
            INIT.call_once(|| {
                nokhwa::nokhwa_initialize(|granted| {
                    if !granted {
                        eprintln!("攝影機權限被拒：請到「系統設定 → 隱私權與安全性 → 相機」允許終端機");
                    }
                });
                // 等使用者回應權限對話框
                for _ in 0..50 {
                    if nokhwa::nokhwa_check() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
        }
    }

    pub fn list_cameras() -> Vec<String> {
        init_permission();
        nokhwa::query(ApiBackend::Auto)
            .map(|v| v.iter().map(|c| c.human_name()).collect())
            .unwrap_or_default()
    }

    pub struct NokhwaCamera {
        rx: Receiver<Result<Frame, String>>,
        stop: Arc<AtomicBool>,
        index: u32,
        live: LiveState,
        error: Option<String>,
    }

    impl NokhwaCamera {
        pub fn open(index: u32) -> Result<Self> {
            init_permission();
            let (tx, rx) = mpsc::sync_channel(2);
            let stop = Arc::new(AtomicBool::new(false));
            let stop2 = stop.clone();
            thread::spawn(move || {
                let fmt = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
                let mut cam = match nokhwa::Camera::new(CameraIndex::Index(index), fmt) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(Err(format!("開不了攝影機：{e}")));
                        return;
                    }
                };
                if let Err(e) = cam.open_stream() {
                    let _ = tx.send(Err(format!("攝影機串流失敗：{e}")));
                    return;
                }
                while !stop2.load(Ordering::Relaxed) {
                    let img = cam.frame().map_err(|e| e.to_string()).and_then(|b| b.decode_image::<RgbAFormat>().map_err(|e| e.to_string()));
                    match img {
                        Ok(img) => {
                            let (w, h) = img.dimensions();
                            let frame = Frame::new(w, h, img.into_raw());
                            // 通道滿了就丟掉這張，只保留最新
                            match tx.try_send(Ok(frame)) {
                                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                                Err(mpsc::TrySendError::Disconnected(_)) => break,
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            break;
                        }
                    }
                }
                let _ = cam.stop_stream();
            });
            Ok(NokhwaCamera { rx, stop, index, live: LiveState { paused: false, last_time: None }, error: None })
        }
    }

    impl Drop for NokhwaCamera {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    impl Source for NokhwaCamera {
        fn label(&self) -> String {
            format!("攝影機 {}", self.index)
        }
        fn poll(&mut self, now: f64) -> Option<(Frame, f32)> {
            let mut newest = None;
            while let Ok(r) = self.rx.try_recv() {
                match r {
                    Ok(f) => newest = Some(f),
                    Err(e) => self.error = Some(e),
                }
            }
            self.live.take(now, newest)
        }
        fn set_paused(&mut self, paused: bool) {
            self.live.paused = paused;
            self.live.last_time = None;
        }
        fn paused(&self) -> bool {
            self.live.paused
        }
        fn is_live(&self) -> bool {
            true
        }
        fn error(&self) -> Option<String> {
            self.error.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_dshow_devices;

    #[test]
    fn parses_dshow_device_list() {
        // 新版格式（每行標類型）
        let new = r#"[in#0 @ 0000] "Integrated Camera" (video)
[in#0 @ 0000]   Alternative name "@device_pnp_\\?\usb#vid"
[in#0 @ 0000] "OBS Virtual Camera" (video)
[in#0 @ 0000] "Microphone Array" (audio)"#;
        assert_eq!(parse_dshow_devices(new), ["Integrated Camera", "OBS Virtual Camera"]);
        // 舊版格式（分段標題）
        let old = r#"[dshow @ 0] DirectShow video devices (some may be both video and audio devices)
[dshow @ 0]  "USB Camera"
[dshow @ 0]     Alternative name "@device_pnp_x"
[dshow @ 0] DirectShow audio devices
[dshow @ 0]  "Mic""#;
        assert_eq!(parse_dshow_devices(old), ["USB Camera"]);
    }
}
