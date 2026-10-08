//! 把內建預設組寫成 presets/*.json（改了 params.rs 的預設值後重跑一次）。
fn main() -> std::io::Result<()> {
    std::fs::create_dir_all("presets")?;
    std::fs::write("presets/default.json", time_echo::params::Params::default().to_json())?;
    std::fs::write("presets/demo-time-memory.json", time_echo::params::Params::demo().to_json())?;
    println!("已寫入 presets/default.json、presets/demo-time-memory.json");
    Ok(())
}
