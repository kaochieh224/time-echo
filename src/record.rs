//! 錄影：把輸出畫面以 rawvideo 餵給 ffmpeg，存成 H.264 MP4。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};

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
        let mut child = crate::source::command(crate::source::ffmpeg_bin())
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{width}x{height}"), "-r", &format!("{fps}"), "-i", "-"])
            .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .context(crate::source::FFMPEG_HINT)?;
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

/// 預設錄影檔名：time-echo-YYYYMMDD-HHMMSS.mp4。
/// macOS／Linux 用 `date` 取本地時間；Windows 沒有這個指令，改用 UTC 並在結尾標 Z。
pub fn timestamp_name() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if !cfg!(windows) {
        let out = crate::source::command("date").arg("+%Y%m%d-%H%M%S").output().ok().filter(|o| o.status.success());
        if let Some(o) = out {
            return format!("time-echo-{}.mp4", String::from_utf8_lossy(&o.stdout).trim());
        }
    }
    format!("time-echo-{}Z.mp4", utc_stamp(secs))
}

/// UNIX 秒數 → YYYYMMDD-HHMMSS（UTC；日期換算用 Howard Hinnant 的 civil_from_days）
fn utc_stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn utc_stamp_matches_known_dates() {
        assert_eq!(super::utc_stamp(0), "19700101-000000");
        assert_eq!(super::utc_stamp(1_791_469_845), "20261008-143045");
        assert_eq!(super::utc_stamp(951_782_400), "20000229-000000");
    }
}
