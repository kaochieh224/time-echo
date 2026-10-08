//! 估一個音訊／影片檔的 BPM：cargo run --example bpm -- 檔案.mp3
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("用法：bpm 檔案"))?;
    let pcm = time_echo::audio::decode(std::path::Path::new(&path))?;
    let secs = pcm.len() as f32 / 2.0 / time_echo::audio::RATE as f32;
    match time_echo::audio::estimate_bpm(&pcm, 2, time_echo::audio::RATE) {
        Some(b) => println!("{path}：{secs:.1} 秒，估計 {b:.1} BPM"),
        None => println!("{path}：{secs:.1} 秒，估不出 BPM（太短或沒有明顯節奏）"),
    }
    Ok(())
}
