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

/// 中間一塊方形人物（遮罩 1）、其餘透明，用來測輪廓、陰影、Selective colour。
fn square_person(colour: [u8; 3]) -> (Frame, Mask) {
    let f = Frame::new(160, 90, [colour[0], colour[1], colour[2], 255].repeat(160 * 90));
    let mut m = Mask::filled(160, 90, 0.0);
    for y in 30..90 {
        for x in 60..100 {
            m.data[y * 160 + x] = 1.0;
        }
    }
    (f, m)
}

fn single_echo() -> Params {
    Params { figure_size: 100.0, echo_count: 1, axis_x: 0.0, edge_softness: 0.0, ..base() }
}

#[test]
fn contour_keeps_only_the_edge() {
    let mut p = single_echo();
    let Some(mut e) = engine(&p) else { return };
    let (f, m) = square_person([200, 50, 50]);
    e.push_frame(&p, &f, Some(&m), 0.1);
    p.surface = Surface::Contour;
    e.render(&p, 0.0, false);
    let out = e.renderer.read_output().unwrap();
    // 方塊在輸出 x 120–200、y 60–180；中心應透明（背景色），左邊緣應是白線
    assert!(close(px(&out, 160, 130), [0x10, 0x20, 0x30]), "內部 {:?}", px(&out, 160, 130));
    let edge = (116..124).map(|x| px(&out, x, 130)).max_by_key(|c| c[0] as u32 + c[1] as u32 + c[2] as u32).unwrap();
    assert!(edge[0] > 150 && edge[1] > 150, "邊緣應有白線 {edge:?}");
}

#[test]
fn ground_shadow_darkens_behind_feet() {
    let mut p = single_echo();
    p.bg_colour = Rgb::hex(0xC0C0C0);
    let Some(mut e) = engine(&p) else { return };
    let (f, m) = square_person([200, 50, 50]);
    e.push_frame(&p, &f, Some(&m), 0.1);
    e.render(&p, 0.0, false);
    let before = e.renderer.read_output().unwrap();
    p.ground_shadows = true;
    p.shadow_opacity = 100.0;
    e.render(&p, 0.0, false);
    let after = e.renderer.read_output().unwrap();
    // 陰影往右上斜：方塊右側、腳附近的背景變暗
    let (x, y) = (215, 170);
    assert!(close(px(&before, x, y), [0xC0, 0xC0, 0xC0]));
    assert!(px(&after, x, y)[0] < 0xB0, "陰影 {:?}", px(&after, x, y));
    // 人物本身仍畫在陰影上面
    assert!(close(px(&after, 160, 130), [200, 50, 50]));
}

#[test]
fn selective_colour_shifts_only_target_hue() {
    let mut p = single_echo();
    let Some(mut e) = engine(&p) else { return };
    // 左半紅、右半藍
    let mut rgba = Vec::new();
    for _y in 0..90 {
        for x in 0..160 {
            rgba.extend_from_slice(if x < 80 { &[220, 30, 30, 255] } else { &[30, 30, 220, 255] });
        }
    }
    let f = Frame::new(160, 90, rgba);
    e.push_frame(&p, &f, Some(&Mask::filled(1, 1, 1.0)), 0.1);
    p.selective = true;
    p.selective_colour = Rgb::hex(0xFF0000);
    p.selective_hue = 120.0; // 紅 → 綠
    e.render(&p, 0.0, false);
    let out = e.renderer.read_output().unwrap();
    let red_side = px(&out, 60, 90);
    assert!(red_side[1] > 150 && red_side[0] < 80, "紅色應轉成綠色 {red_side:?}");
    assert!(close(px(&out, 260, 90), [30, 30, 220]), "藍色不受影響");
}

#[test]
fn keylight_despill_and_mask() {
    use time_echo::params::MatteSource;
    use time_echo::segment::{SegJob, build_mask};
    let mut p = single_echo();
    p.matte_source = MatteSource::Key;
    p.screen_colour = Rgb::hex(0x00B140);
    p.despill = 100.0;
    let Some(mut e) = engine(&p) else { return };
    // 左半綠幕、右半帶綠邊的膚色
    let mut rgba = Vec::new();
    for _y in 0..90 {
        for x in 0..160 {
            rgba.extend_from_slice(if x < 80 { &[0, 177, 64, 255] } else { &[200, 180, 140, 255] });
        }
    }
    let f = std::sync::Arc::new(Frame::new(160, 90, rgba));
    let mask = build_mask(None, &SegJob::from_params(f.clone(), 160, 90, &p)).unwrap();
    assert!(mask.get(20, 45) < 0.05 && mask.get(140, 45) > 0.9);
    e.push_frame(&p, &f, Some(&mask), 0.1);
    e.render(&p, 0.0, false);
    let out = e.renderer.read_output().unwrap();
    assert!(close(px(&out, 60, 90), [0x10, 0x20, 0x30]), "綠幕應被去掉");
    let with = px(&out, 260, 90);
    // 同一張影格不做 Despill 再擷取一次比較
    p.despill = 0.0;
    e.push_frame(&p, &f, Some(&mask), 0.1);
    e.render(&p, 0.0, false);
    let without = px(&e.renderer.read_output().unwrap(), 260, 90);
    assert!(without[1] >= with[1] + 6, "Despill 應壓低綠色：{with:?} vs {without:?}");
    assert!(close([with[0], 0, with[2]], [without[0], 0, without[2]]), "紅藍不變");
}
