//! GPU 管線整合測試：用合成影格（每張一個已知顏色）跑擷取 → 記憶緩衝 → 合成，
//! 讀回輸出逐點檢查。沒有 GPU（含軟體 GPU）的環境會略過。

use time_echo::engine::Engine;
use time_echo::frame::{Frame, Mask};
use time_echo::params::{Params, Rgb, Surface};
use time_echo::render::Renderer;

fn engine(p: &Params) -> Option<Engine> {
    match time_echo::headless::create_device() {
        Ok((d, q, _)) => Some(Engine::new(Renderer::new(d, q), p)),
        Err(e) => {
            eprintln!("略過 GPU 測試：{e}");
            None
        }
    }
}

/// 第 k 張影格的顏色
fn colour(k: u32) -> [u8; 3] {
    [(k * 20 % 256) as u8, (250 - k * 20 % 250) as u8, 90]
}

fn frame(k: u32) -> Frame {
    let [r, g, b] = colour(k);
    Frame::new(160, 90, [r, g, b, 255].repeat(160 * 90))
}

fn base() -> Params {
    Params {
        output_res: 320,
        mirror: false,
        echo_count: 4,
        interval: 0.2,
        capture_rate: 10.0,
        memory_length: 4.0,
        spacing: 100.0,
        figure_size: 20.0,
        axis_x: 50.0,
        bg_colour: Rgb::hex(0x102030),
        edge_softness: 0.0,
        ..Params::default()
    }
}

fn px(f: &Frame, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * f.width + x) * 4) as usize;
    [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2]]
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= 3)
}

fn push(e: &mut Engine, p: &Params, ks: std::ops::Range<u32>, mask: &Mask) {
    for k in ks {
        e.push_frame(p, &frame(k), Some(mask), 0.1);
    }
}

/// 分身 i 的中心點（輸出 320×180、分身 64×36、底部對齊）
fn echo_centre(i: u32) -> (u32, u32) {
    let step = 320.0 / 3.0;
    ((240.0 - i as f32 * step).round() as u32, 162)
}

#[test]
fn echoes_show_past_frames_at_expected_positions() {
    let p = base();
    let Some(mut e) = engine(&p) else { return };
    let full = Mask::filled(1, 1, 1.0);
    push(&mut e, &p, 0..10, &full);
    let (w, h) = e.render(&p, 0.0, true);
    assert_eq!((w, h), (320, 180));
    let out = e.renderer.read_output().unwrap();

    // 背景色
    assert!(close(px(&out, 2, 2), [0x10, 0x20, 0x30]), "背景 {:?}", px(&out, 2, 2));
    // i = 0、1、2 分別是第 9、7、5 張（偏移 0、2、4）；i = 3 在畫面外
    for (i, k) in [(0, 9), (1, 7), (2, 5)] {
        let (x, y) = echo_centre(i);
        assert!(close(px(&out, x, y), colour(k)), "分身 {i} 應為第 {k} 張：{:?} vs {:?}", px(&out, x, y), colour(k));
    }
    assert_eq!(e.last_plan.instances.len(), 4);
    // 分身上方仍是背景（腳貼齊底部、尺寸 20%）
    assert!(close(px(&out, 240, 100), [0x10, 0x20, 0x30]));
}

#[test]
fn pause_freezes_and_reset_restarts() {
    let p = base();
    let Some(mut e) = engine(&p) else { return };
    let full = Mask::filled(1, 1, 1.0);
    push(&mut e, &p, 0..10, &full);
    e.render(&p, 0.0, false);
    let a = e.renderer.read_output().unwrap();
    // 暫停 = 不再送影格：再算幾次畫面不變
    e.render(&p, 1.0, false);
    let b = e.renderer.read_output().unwrap();
    assert_eq!(a.rgba, b.rgba);

    e.reset_memory(&p);
    push(&mut e, &p, 20..21, &full);
    e.render(&p, 2.0, false);
    let c = e.renderer.read_output().unwrap();
    assert_eq!(e.last_plan.instances.len(), 1, "Reset 後只剩最新一個分身");
    let (x, y) = echo_centre(0);
    assert!(close(px(&c, x, y), colour(20)));
    let (x1, y1) = echo_centre(1);
    assert!(close(px(&c, x1, y1), [0x10, 0x20, 0x30]), "舊分身應已清掉");
}

#[test]
fn clip_and_disintegrate_and_solid() {
    let mut p = base();
    let Some(mut e) = engine(&p) else { return };
    // 遮罩 0.5，Clip black 60% → 人物全透明
    let half = Mask::filled(4, 4, 0.5);
    push(&mut e, &p, 0..10, &half);
    p.clip_black = 60.0;
    e.render(&p, 0.0, false);
    let out = e.renderer.read_output().unwrap();
    let (x, y) = echo_centre(0);
    assert!(close(px(&out, x, y), [0x10, 0x20, 0x30]));
    // Clip white 40% → 0.5 變成完全不透明
    p.clip_black = 0.0;
    p.clip_white = 40.0;
    e.render(&p, 0.0, false);
    let out = e.renderer.read_output().unwrap();
    assert!(close(px(&out, x, y), colour(9)));

    // Disintegrate 100%：最新完整、最舊（i = 3 → 改 N = 3 讓 i = 2 在畫面內）全侵蝕
    p.echo_count = 3;
    p.disintegrate = 100.0;
    p.spacing = 100.0;
    e.render(&p, 0.0, false);
    let out = e.renderer.read_output().unwrap();
    // N = 3：間距 160，最新在 240，最舊在 -80（畫面外）；中間的 i = 1 侵蝕一半
    let newest = px(&out, 240, 162);
    assert!(close(newest, colour(9)), "最新分身不該被侵蝕：{newest:?}");
    let mut eroded = 0;
    let mut total = 0;
    for y in 150..178 {
        for x in 60..100 {
            total += 1;
            if close(px(&out, x, y), [0x10, 0x20, 0x30]) {
                eroded += 1;
            }
        }
    }
    let ratio = eroded as f32 / total as f32;
    assert!(ratio > 0.15 && ratio < 0.85, "中間分身應部分侵蝕，實際 {ratio}");

    // 實心剪影：填白色
    p.disintegrate = 0.0;
    p.surface = Surface::Solid;
    e.render(&p, 0.0, false);
    let out = e.renderer.read_output().unwrap();
    assert!(close(px(&out, 240, 162), [255, 255, 255]));
}

#[test]
fn grading_duotone_bloom_mirror_do_what_they_say() {
    let mut p = base();
    let Some(mut e) = engine(&p) else { return };
    // 左半黑、右半白的影格，用來測 mirror 與雙色調
    let mut rgba = Vec::new();
    for _y in 0..90 {
        for x in 0..160 {
            let v = if x < 80 { 0 } else { 255 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let f = Frame::new(160, 90, rgba);
    p.figure_size = 100.0;
    p.echo_count = 1;
    p.axis_x = 0.0;
    p.duotone = true;
    p.duotone_dark = Rgb::hex(0x0000FF);
    p.duotone_light = Rgb::hex(0xFF0000);
    for _ in 0..3 {
        e.push_frame(&p, &f, Some(&Mask::filled(1, 1, 1.0)), 0.1);
    }
    e.render(&p, 0.0, true);
    let out = e.renderer.read_output().unwrap();
    assert!(close(px(&out, 40, 90), [0, 0, 255]), "暗部 → 暗部色 {:?}", px(&out, 40, 90));
    assert!(close(px(&out, 280, 90), [255, 0, 0]), "亮部 → 亮部色 {:?}", px(&out, 280, 90));

    // Mirror 只影響之後擷取的影格
    p.mirror = true;
    e.push_frame(&p, &f, None, 0.1);
    e.render(&p, 0.0, false);
    let out = e.renderer.read_output().unwrap();
    assert!(close(px(&out, 40, 90), [255, 0, 0]), "鏡像後左邊應是亮部");

    // 亮度 −100%：rgb − 0.5，白變中灰 → 雙色調正中間；黑仍是暗部色
    p.brightness = -100.0;
    e.render(&p, 0.0, false);
    let out = e.renderer.read_output().unwrap();
    assert!(close(px(&out, 40, 90), [128, 0, 128]), "{:?}", px(&out, 40, 90));
    assert!(close(px(&out, 280, 90), [0, 0, 255]));

    // Bloom：亮部周圍的背景會被照亮
    p.duotone = false;
    p.brightness = 0.0;
    p.figure_size = 50.0;
    p.bg_colour = Rgb::hex(0x000000);
    e.render(&p, 0.0, false);
    let dark = e.renderer.read_output().unwrap();
    p.bloom = 100.0;
    e.render(&p, 0.0, false);
    let glow = e.renderer.read_output().unwrap();
    let sum = |f: &Frame| f.rgba.iter().map(|v| *v as u64).sum::<u64>();
    assert!(sum(&glow) > sum(&dark) + 100_000, "Bloom 應讓畫面變亮");
    assert!(e.renderer.monitor_views().is_some());
}
