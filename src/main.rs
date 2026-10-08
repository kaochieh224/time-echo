//! time-echo：時間分身特效控制器
//!
//! 用法：
//!   time-echo [影片檔] [--demo] [--preset 檔案.json]          開啟控制器
//!   time-echo render -i 輸入.mp4 -o 輸出.mp4 [選項]           離線算圖（不開視窗）
//!     --demo | --preset 檔案.json   參數預設組
//!     --matte ai|luma|full          遮罩來源（預設 ai）
//!     --model 模型.onnx             指定分割模型
//!     --frames N                    只算前 N 張
//!     --mirror                      左右鏡像（影片預設不鏡像）

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use time_echo::params::{MatteSource, Params};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("render") {
        return render(&args[1..]);
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{}", include_str!("main.rs").lines().skip(2).take(9).map(|l| l.trim_start_matches("//!")).collect::<Vec<_>>().join("\n"));
        return Ok(());
    }
    let mut params = Params::default();
    let mut video = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--demo" => params = Params::demo(),
            "--preset" => params = load_preset(it.next())?,
            other if !other.starts_with('-') => video = Some(PathBuf::from(other)),
            other => bail!("不認得的參數：{other}"),
        }
    }
    time_echo::app::run(time_echo::app::StartOptions { video, params }).map_err(|e| anyhow!("{e}"))
}

fn load_preset(path: Option<&String>) -> Result<Params> {
    let path = path.ok_or_else(|| anyhow!("--preset 後面要接 JSON 檔"))?;
    let text = std::fs::read_to_string(path).with_context(|| format!("讀不了 {path}"))?;
    Params::from_json(&text)
}

fn render(args: &[String]) -> Result<()> {
    let (mut input, mut output, mut model, mut frames) = (None, None, None, None);
    let mut params = Params::default();
    let mut matte = None;
    let mut mirror = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| anyhow!("{a} 後面缺少值"));
        match a.as_str() {
            "-i" | "--input" => input = Some(PathBuf::from(val()?)),
            "-o" | "--output" => output = Some(PathBuf::from(val()?)),
            "--model" => model = Some(PathBuf::from(val()?)),
            "--frames" => frames = Some(val()?.parse::<u64>().context("--frames 要是整數")?),
            "--demo" => params = Params::demo(),
            "--mirror" => mirror = true,
            "--preset" => params = load_preset(Some(&val()?))?,
            "--matte" => {
                matte = Some(match val()?.as_str() {
                    "ai" => MatteSource::Ai,
                    "luma" => MatteSource::Luma,
                    "full" => MatteSource::Full,
                    o => bail!("--matte 只能是 ai／luma／full，收到 {o}"),
                })
            }
            o => bail!("不認得的參數：{o}"),
        }
    }
    if let Some(m) = matte {
        params.matte_source = m;
    }
    let input = input.ok_or_else(|| anyhow!("缺少 -i 輸入影片"))?;
    let output = output.unwrap_or_else(|| PathBuf::from("time-echo-out.mp4"));
    eprintln!("算圖：{} → {}", input.display(), output.display());
    let stats = time_echo::headless::run(
        time_echo::headless::RenderOptions { input, output: output.clone(), params, model, max_frames: frames, mirror },
        |n| {
            if n % 30 == 0 {
                eprint!("\r  {n} 張");
            }
        },
    )?;
    eprintln!(
        "\r完成：{} 張（擷取進記憶 {} 張），{:.1} 秒；GPU：{}；遮罩：{}；最後一張畫出 {} 個分身",
        stats.frames, stats.captured, stats.seconds, stats.adapter, stats.segmenter, stats.last_visible_echoes
    );
    Ok(())
}
