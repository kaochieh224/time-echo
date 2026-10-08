//! 錄影：把輸出畫面以 rawvideo 餵給 ffmpeg，存成 H.264 MP4。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use anyhow::{Context, Result};

use crate::frame::Frame;

pub struct Recorder {
    child: Child,
    stdin: Option<ChildStdin>,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub frames: u64,
}

impl Recorder {
    pub fn start(path: &Path, width: u32, height: u32, fps: f32) -> Result<Self> {
        let mut child = Command::new(crate::source::ffmpeg_bin())
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{width}x{height}"), "-r", &format!("{fps}"), "-i", "-"])
            .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .context("找不到 ffmpeg（請先安裝：brew install ffmpeg）")?;
        let stdin = child.stdin.take();
        Ok(Recorder { child, stdin, path: path.to_path_buf(), width, height, fps, frames: 0 })
    }

    /// 尺寸不同的影格（中途改輸出解析度）直接略過。
    pub fn write(&mut self, frame: &Frame) -> Result<()> {
        if frame.width != self.width || frame.height != self.height {
            return Ok(());
        }
        self.stdin.as_mut().context("錄影已結束")?.write_all(&frame.rgba).context("寫入 ffmpeg 失敗")?;
        self.frames += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<PathBuf> {
        drop(self.stdin.take());
        let status = self.child.wait()?;
        anyhow::ensure!(status.success(), "ffmpeg 編碼失敗（{status}）");
        Ok(self.path.clone())
    }
}

/// 預設錄影檔名：time-echo-YYYYMMDD-HHMMSS.mp4（本地時間取不到時用 UNIX 秒數）。
pub fn timestamp_name() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let out = Command::new("date").arg("+%Y%m%d-%H%M%S").output().ok().filter(|o| o.status.success());
    match out {
        Some(o) => format!("time-echo-{}.mp4", String::from_utf8_lossy(&o.stdout).trim()),
        None => format!("time-echo-{secs}.mp4"),
    }
}
