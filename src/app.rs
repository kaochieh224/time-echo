//! 桌面控制器介面（eframe／egui）。主畫面置中，左側監看小窗，右側 01–05 控制面板。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, TextureId};
use eframe::egui_wgpu;

use crate::engine::Engine;
use crate::frame::Mask;
use crate::params::{BgMode, ColourMode, Layout, MatteSource, Params, Surface};
use crate::record::{self, Recorder};
use crate::render::Renderer;
use crate::segment::{self, SegJob, SegWorker};
use crate::source::{self, Source, VideoSource};

const INK: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
const RED: Color32 = Color32::from_rgb(0xd4, 0x10, 0x1a);
const HAIRLINE: Color32 = Color32::from_rgb(0xd8, 0xd8, 0xd8);
const MUTED: Color32 = Color32::from_rgb(0x70, 0x70, 0x70);
const RECORD_FPS: f32 = 30.0;

pub struct StartOptions {
    pub video: Option<PathBuf>,
    pub params: Params,
}

pub struct TimeEchoApp {
    p: Params,
    engine: Engine,
    render_state: egui_wgpu::RenderState,
    source: Option<Box<dyn Source>>,
    seg: SegWorker,
    start: Instant,
    out_tex: Option<TextureId>,
    mon_tex: Option<[TextureId; 3]>,
    tex_generation: u64,
    hide_ui: bool,
    show_guide: bool,
    recorder: Option<Recorder>,
    rec_next: f64,
    message: String,
    frame_times: VecDeque<f64>,
    cameras: Vec<String>,
    camera_index: u32,
    ffmpeg_camera: String,
    model_name: Option<String>,
}

pub fn run(opts: StartOptions) -> eframe::Result {
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1600.0, 940.0]).with_title("TIME ECHO · 時間分身控制器"),
        ..Default::default()
    };
    eframe::run_native("time-echo", native, Box::new(move |cc| Ok(Box::new(TimeEchoApp::new(cc, opts)?))))
}

impl TimeEchoApp {
    fn new(cc: &eframe::CreationContext<'_>, opts: StartOptions) -> anyhow::Result<Self> {
        setup_style(&cc.egui_ctx);
        let rs = cc.wgpu_render_state.clone().ok_or_else(|| anyhow::anyhow!("需要 wgpu 繪圖後端"))?;
        let renderer = Renderer::new(rs.device.clone(), rs.queue.clone());
        let engine = Engine::new(renderer, &opts.params);
        let seg = SegWorker::spawn();
        let mut message = String::new();
        if segment::ai_available() {
            match segment::find_model() {
                Some(m) => seg.load_model(m),
                None => {
                    message = format!(
                        "找不到 AI 分割模型（models/{}）。先執行 scripts/setup_macos.sh，或改用「亮度」遮罩。",
                        segment::DEFAULT_MODEL_FILE
                    )
                }
            }
        }
        let mut app = TimeEchoApp {
            p: opts.params,
            engine,
            render_state: rs,
            source: None,
            seg,
            start: Instant::now(),
            out_tex: None,
            mon_tex: None,
            tex_generation: u64::MAX,
            hide_ui: false,
            show_guide: false,
            recorder: None,
            rec_next: 0.0,
            message,
            frame_times: VecDeque::new(),
            cameras: Vec::new(),
            camera_index: 0,
            ffmpeg_camera: "0".into(),
            model_name: None,
        };
        if let Some(v) = opts.video {
            app.open_video(&v);
        }
        Ok(app)
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn set_source(&mut self, s: Box<dyn Source>) {
        // 換來源時自動套用 Mirror 預設：攝影機開、影片關
        self.p.mirror = s.is_live();
        self.message = format!("來源：{}", s.label());
        self.source = Some(s);
        self.seg.reset();
        self.engine.reset_memory(&self.p);
    }

    fn open_video(&mut self, path: &Path) {
        match VideoSource::open(path, true) {
            Ok(v) => self.set_source(Box::new(v)),
            Err(e) => self.message = format!("無法開啟影片：{e:#}"),
        }
    }

    fn open_camera(&mut self) {
        #[cfg(feature = "camera")]
        match source::NokhwaCamera::open(self.camera_index) {
            Ok(c) => self.set_source(Box::new(c)),
            Err(e) => self.message = format!("無法開啟攝影機：{e:#}"),
        }
        #[cfg(not(feature = "camera"))]
        {
            self.message = "這個版本沒有編入 nokhwa 攝影機，請用「攝影機（ffmpeg）」".into();
        }
    }

    fn open_ffmpeg_camera(&mut self) {
        match source::FfmpegCamera::open(&self.ffmpeg_camera) {
            Ok(c) => self.set_source(Box::new(c)),
            Err(e) => self.message = format!("無法開啟攝影機：{e:#}"),
        }
    }

    fn toggle_pause(&mut self) {
        if let Some(s) = self.source.as_mut() {
            let p = !s.paused();
            s.set_paused(p);
        }
    }

    fn toggle_record(&mut self) {
        if let Some(rec) = self.recorder.take() {
            let frames = rec.frames;
            self.message = match rec.finish() {
                Ok(path) => format!("已存檔：{}（{frames} 張）", path.display()),
                Err(e) => format!("錄影失敗：{e:#}"),
            };
            return;
        }
        let Some((w, h)) = self.engine.renderer.output_size() else {
            self.message = "還沒有畫面可以錄".into();
            return;
        };
        let dir = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Movies"))
            .filter(|d| d.is_dir())
            .unwrap_or_else(|| PathBuf::from("."));
        let path = dir.join(record::timestamp_name());
        match Recorder::start(&path, w, h, RECORD_FPS) {
            Ok(r) => {
                self.message = format!("錄影中 → {}", path.display());
                self.recorder = Some(r);
                self.rec_next = self.now();
            }
            Err(e) => self.message = format!("無法開始錄影：{e:#}"),
        }
    }

    fn step_pipeline(&mut self) {
        let now = self.now();
        if let Some(m) = self.seg.take_new() {
            self.engine.update_mask(&m);
        }
        let polled = self.source.as_mut().and_then(|s| s.poll(now));
        if let Some((frame, dt)) = polled {
            let frame = Arc::new(frame);
            let full = self.p.matte_source == MatteSource::Full;
            if !full {
                let (bw, bh) = self.engine.buffer_dims(frame.width, frame.height);
                self.seg.submit(SegJob {
                    frame: frame.clone(),
                    width: bw,
                    height: bh,
                    source: self.p.matte_source,
                    shrink_grow: self.p.shrink_grow,
                    softness: self.p.edge_softness,
                    px_scale: 1.0,
                });
            }
            let white = full.then(|| Mask::filled(1, 1, 1.0));
            self.engine.push_frame(&self.p, &frame, white.as_ref(), dt);
        }
        if let Some(e) = self.source.as_ref().and_then(|s| s.error()) {
            self.message = format!("來源錯誤：{e}");
        }
        self.engine.render(&self.p, now, self.p.monitors);

        // 錄影：以 30 fps 牆上時間取樣輸出，畫面掉幀時重複上一張
        if let Some(rec) = self.recorder.as_mut() {
            if now >= self.rec_next {
                if let Some(out) = self.engine.renderer.read_output() {
                    let mut n = 0;
                    while now >= self.rec_next && n < 4 {
                        if let Err(e) = rec.write(&out) {
                            self.message = format!("錄影錯誤：{e:#}");
                        }
                        self.rec_next += 1.0 / RECORD_FPS as f64;
                        n += 1;
                    }
                    if now - self.rec_next > 0.5 {
                        self.rec_next = now;
                    }
                }
            }
        }

        self.frame_times.push_back(now);
        while self.frame_times.len() > 2 && now - self.frame_times[0] > 1.0 {
            self.frame_times.pop_front();
        }
    }

    fn sync_textures(&mut self) {
        let r = &self.engine.renderer;
        if r.generation == self.tex_generation {
            return;
        }
        let mut egui_r = self.render_state.renderer.write();
        let dev = &self.render_state.device;
        if let Some(id) = self.out_tex.take() {
            egui_r.free_texture(&id);
        }
        for id in self.mon_tex.take().into_iter().flatten() {
            egui_r.free_texture(&id);
        }
        self.out_tex = r.output_view().map(|v| egui_r.register_native_texture(dev, v, eframe::wgpu::FilterMode::Linear));
        self.mon_tex = r.monitor_views().map(|vs| vs.map(|v| egui_r.register_native_texture(dev, v, eframe::wgpu::FilterMode::Linear)));
        self.tex_generation = r.generation;
    }

    fn output_fps(&self) -> f32 {
        match (self.frame_times.front(), self.frame_times.back()) {
            (Some(a), Some(b)) if b > a => (self.frame_times.len() - 1) as f32 / (b - a) as f32,
            _ => 0.0,
        }
    }

    fn handle_keys(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (h, f, r, space, shift, esc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::H),
                i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::R),
                i.key_pressed(egui::Key::Space),
                i.modifiers.shift,
                i.key_pressed(egui::Key::Escape),
            )
        });
        if h {
            self.hide_ui = !self.hide_ui;
        }
        if f {
            let fs = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
        }
        if esc {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            self.hide_ui = false;
        }
        if r && shift {
            self.toggle_record();
        } else if r {
            self.engine.reset_memory(&self.p);
            self.seg.reset();
        }
        if space {
            self.toggle_pause();
        }
    }
}

impl eframe::App for TimeEchoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_keys(ui);
        self.step_pipeline();
        self.sync_textures();
        let (status, model, ai_fps) = self.seg.status();
        if model.is_some() || !status.is_empty() {
            self.model_name = model;
        }

        if !self.hide_ui {
            egui::Panel::top("status").show(ui, |ui| self.status_bar(ui, &status, ai_fps));
            egui::Panel::right("controls").resizable(false).exact_size(360.0).show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
            });
            if self.p.monitors {
                egui::Panel::left("monitors").resizable(false).exact_size(200.0).show(ui, |ui| self.monitors(ui));
            }
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
            if let (Some(id), Some((w, h))) = (self.out_tex, self.engine.renderer.output_size()) {
                let s = (rect.width() / w as f32).min(rect.height() / h as f32);
                let img = egui::Rect::from_center_size(rect.center(), egui::vec2(w as f32 * s, h as f32 * s));
                ui.painter().image(id, img, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
            if !self.engine.has_source() && !self.hide_ui {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "選擇來源：Start camera 或 Load video",
                    egui::FontId::proportional(18.0),
                    Color32::from_gray(160),
                );
            }
        });
        if self.show_guide {
            self.guide_window(ui);
        }
        ui.ctx().request_repaint();
    }

    fn on_exit(&mut self) {
        if let Some(rec) = self.recorder.take() {
            let _ = rec.finish();
        }
    }
}

// ───────────── 介面 ─────────────

impl TimeEchoApp {
    fn status_bar(&mut self, ui: &mut egui::Ui, status: &str, ai_fps: f32) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("TIME ECHO").strong().color(INK));
            ui.separator();
            let src = self.source.as_ref().map(|s| s.label()).unwrap_or_else(|| "無來源".into());
            let paused = self.source.as_ref().is_some_and(|s| s.paused());
            ui.label(if paused { format!("{src} · 暫停") } else { src });
            ui.separator();
            ui.label(format!("{:.0} FPS", self.output_fps()));
            if self.p.matte_source != MatteSource::Full {
                ui.label(format!("{ai_fps:.0} AI FPS"));
            }
            ui.label(format!("{} / {} ECHO FRAMES", self.engine.ring.count(), self.engine.ring.capacity()));
            if let Some(rec) = &self.recorder {
                ui.label(RichText::new(format!("● REC {:.0}s", rec.frames as f32 / RECORD_FPS)).color(RED).strong());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Guide").clicked() {
                    self.show_guide = !self.show_guide;
                }
                if ui.button("Fullscreen  F").clicked() {
                    let fs = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
                }
                if ui.button("Hide UI  H").clicked() {
                    self.hide_ui = true;
                }
            });
        });
        let plan = &self.engine.last_plan;
        let mut notes: Vec<String> = Vec::new();
        if plan.beyond_capacity > 0 {
            notes.push(format!("有 {} 個分身超出記憶長度", plan.beyond_capacity));
        }
        if plan.interval_too_small {
            notes.push("間隔小於取樣間隔，相鄰分身會取到同一張".into());
        }
        if self.engine.buffer_settings_pending(&self.p) {
            notes.push("緩衝設定已改，按 Reset memory 生效".into());
        }
        let line = if !self.message.is_empty() { self.message.clone() } else { status.to_string() };
        ui.horizontal(|ui| {
            ui.label(RichText::new(line).small().color(MUTED));
            for n in notes {
                ui.label(RichText::new(n).small().color(RED));
            }
        });
    }

    fn monitors(&self, ui: &mut egui::Ui) {
        let (Some(ids), Some((w, h))) = (self.mon_tex, self.engine.renderer.monitor_size()) else {
            ui.label(RichText::new("監看小窗：等待來源").small().color(MUTED));
            return;
        };
        let width = ui.available_width();
        for (id, name) in ids.iter().zip(["原始畫面", "人物＋棋盤格", "黑白遮罩"]) {
            ui.label(RichText::new(name).small().color(MUTED));
            ui.image((*id, egui::vec2(width, width * h as f32 / w as f32)));
            ui.add_space(6.0);
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;

        section(ui, "輸出與預設組", true, |ui| {
            ui.horizontal(|ui| {
                ui.label("輸出長邊");
                for r in [960, 1280, 1920] {
                    ui.selectable_value(&mut p.output_res, r, r.to_string());
                }
            });
            ui.horizontal(|ui| {
                if ui.button("預設").clicked() {
                    *p = Params { mirror: p.mirror, ..Params::default() };
                }
                if ui.button("示範 TIME & MEMORY").clicked() {
                    *p = Params { mirror: p.mirror, ..Params::demo() };
                }
            });
            ui.horizontal(|ui| {
                if ui.button("匯入 JSON…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("預設組", &["json"]).pick_file() {
                        match std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|s| Params::from_json(&s)) {
                            Ok(np) => *p = np,
                            Err(e) => self.message = format!("讀不了預設組：{e:#}"),
                        }
                    }
                }
                if ui.button("匯出 JSON…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().set_file_name("time-echo-preset.json").save_file() {
                        if let Err(e) = std::fs::write(&path, p.to_json()) {
                            self.message = format!("存檔失敗：{e}");
                        }
                    }
                }
            });
        });

        // 錄影按鈕放在 section 外，因為要借 self
        ui.horizontal(|ui| {
            let label = if self.recorder.is_some() { "■ 停止錄影  ⇧R" } else { "● 開始錄影  ⇧R" };
            let btn = egui::Button::new(RichText::new(label).color(if self.recorder.is_some() { Color32::WHITE } else { RED }))
                .fill(if self.recorder.is_some() { RED } else { Color32::WHITE });
            if ui.add(btn).clicked() {
                self.toggle_record();
            }
            ui.label(RichText::new("MP4 存到「影片」資料夾").small().color(MUTED));
        });
        ui.add_space(4.0);

        self.source_section(ui);
        let p = &mut self.p;

        section(ui, "02 Choreography 時間分身", true, |ui| {
            grid(ui, "chor", |ui| {
                ui.label("Echo figures");
                let mut n = p.echo_count as f32;
                if ui.add(egui::Slider::new(&mut n, 1.0..=48.0).step_by(1.0).fixed_decimals(0)).changed() {
                    p.echo_count = n as u32;
                }
                ui.end_row();
                ui.label("Temporal interval");
                ui.add_enabled(!p.beat_sync, egui::Slider::new(&mut p.interval, 0.05..=2.0).step_by(0.01).suffix(" s"));
                ui.end_row();
                if p.beat_sync {
                    ui.label("");
                    ui.label(RichText::new(format!("節拍同步中：{:.3} s", p.effective_interval())).small().color(RED));
                    ui.end_row();
                }
                slider(ui, "Echo spacing", &mut p.spacing, -300.0..=300.0, 1.0, " %");
                slider(ui, "Figure size", &mut p.figure_size, 10.0..=300.0, 1.0, " %");
                slider(ui, "Axis X", &mut p.axis_x, -100.0..=100.0, 1.0, "");
                ui.label("排列模式");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut p.layout, Layout::Procession, "行進");
                    ui.selectable_value(&mut p.layout, Layout::Centered, "置中");
                });
                ui.end_row();
                slider(ui, "舊影淡出", &mut p.echo_fade, 0.0..=100.0, 1.0, " %");
            });
        });

        section(ui, "03 Surface & Light 表面與光", true, |ui| {
            grid(ui, "surf", |ui| {
                ui.label("表面模式");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut p.surface, Surface::Textured, "影像紋理");
                    ui.selectable_value(&mut p.surface, Surface::Solid, "實心剪影");
                });
                ui.end_row();
                slider(ui, "Disintegrate", &mut p.disintegrate, 0.0..=100.0, 1.0, " %");
                slider(ui, "Light bloom", &mut p.bloom, 0.0..=100.0, 1.0, " %");
                ui.label("色彩模式");
                egui::ComboBox::from_id_salt("colour_mode")
                    .selected_text(match p.colour_mode {
                        ColourMode::Original => "原色",
                        ColourMode::White => "白色",
                        ColourMode::Mono => "單色（灰階）",
                        ColourMode::Custom => "自訂色",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut p.colour_mode, ColourMode::Original, "原色");
                        ui.selectable_value(&mut p.colour_mode, ColourMode::White, "白色");
                        ui.selectable_value(&mut p.colour_mode, ColourMode::Mono, "單色（灰階）");
                        ui.selectable_value(&mut p.colour_mode, ColourMode::Custom, "自訂色");
                    });
                ui.end_row();
                ui.label("Custom colour");
                ui.color_edit_button_srgb(&mut p.custom_colour.0);
                ui.end_row();
                ui.label("背景");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut p.bg_mode, BgMode::Solid, "純色");
                    ui.selectable_value(&mut p.bg_mode, BgMode::Source, "原始畫面");
                });
                ui.end_row();
                ui.label("Background colour");
                ui.color_edit_button_srgb(&mut p.bg_colour.0);
                ui.end_row();
            });
        });

        section(ui, "04 Colour + Curve 調色（只作用在人物）", false, |ui| {
            grid(ui, "grade", |ui| {
                slider(ui, "Brightness", &mut p.brightness, -100.0..=100.0, 1.0, " %");
                slider(ui, "Contrast", &mut p.contrast, 0.0..=200.0, 1.0, " %");
                slider(ui, "Saturation", &mut p.saturation, 0.0..=200.0, 1.0, " %");
                slider(ui, "Shadows", &mut p.curve_shadows, -100.0..=100.0, 1.0, " %");
                slider(ui, "Midtones", &mut p.curve_mids, -100.0..=100.0, 1.0, " %");
                slider(ui, "Highlights", &mut p.curve_highs, -100.0..=100.0, 1.0, " %");
                ui.label("Duotone 雙色調");
                ui.checkbox(&mut p.duotone, "開啟（蓋過色彩模式）");
                ui.end_row();
                ui.label("暗部色／亮部色");
                ui.horizontal(|ui| {
                    ui.color_edit_button_srgb(&mut p.duotone_dark.0);
                    ui.color_edit_button_srgb(&mut p.duotone_light.0);
                });
                ui.end_row();
            });
            if ui.button("調色歸零").clicked() {
                let d = Params::default();
                (p.brightness, p.contrast, p.saturation) = (d.brightness, d.contrast, d.saturation);
                (p.curve_shadows, p.curve_mids, p.curve_highs) = (0.0, 0.0, 0.0);
                p.duotone = false;
            }
        });

        section(ui, "05 Matte 遮罩整理", false, |ui| {
            grid(ui, "matte", |ui| {
                slider(ui, "Clip black", &mut p.clip_black, 0.0..=99.0, 1.0, " %");
                slider(ui, "Clip white", &mut p.clip_white, 1.0..=100.0, 1.0, " %");
                slider(ui, "Shrink / grow", &mut p.shrink_grow, -10.0..=10.0, 1.0, " px");
                slider(ui, "Edge softness", &mut p.edge_softness, 0.0..=10.0, 0.5, " px");
            });
            if p.clip_white < p.clip_black + 1.0 {
                p.clip_white = (p.clip_black + 1.0).min(100.0);
            }
            ui.label(RichText::new("Shrink／Softness 只影響之後擷取的影格，舊分身要等緩衝輪替完才更新。").small().color(MUTED));
        });

        self.beat_section(ui);
    }

    fn source_section(&mut self, ui: &mut egui::Ui) {
        let mut action: Option<&str> = None;
        let pending = self.engine.buffer_settings_pending(&self.p);
        let model_name = self.model_name.clone();
        let paused = self.source.as_ref().is_some_and(|s| s.paused());
        let p = &mut self.p;
        let cameras = &self.cameras;
        let camera_index = &mut self.camera_index;
        let ff_cam = &mut self.ffmpeg_camera;
        section(ui, "01 Live Input 來源", true, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Start camera").clicked() {
                    action = Some("camera");
                }
                if ui.button("Load video…").clicked() {
                    action = Some("video");
                }
                if ui.button(if paused { "Resume" } else { "Pause" }).clicked() {
                    action = Some("pause");
                }
                if ui.button("Stop input").clicked() {
                    action = Some("stop");
                }
                if ui.button(RichText::new("Reset memory  R").color(if pending { RED } else { INK })).clicked() {
                    action = Some("reset");
                }
            });
            ui.horizontal(|ui| {
                ui.label("攝影機");
                if cameras.is_empty() {
                    ui.add(egui::DragValue::new(camera_index).range(0..=9).prefix("#"));
                } else {
                    egui::ComboBox::from_id_salt("cam")
                        .selected_text(cameras.get(*camera_index as usize).cloned().unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for (i, n) in cameras.iter().enumerate() {
                                ui.selectable_value(camera_index, i as u32, n);
                            }
                        });
                }
                if ui.small_button("列出").clicked() {
                    action = Some("list");
                }
            });
            ui.horizontal(|ui| {
                ui.label("備援");
                ui.add(egui::TextEdit::singleline(ff_cam).desired_width(30.0));
                if ui.button("攝影機（ffmpeg）").clicked() {
                    action = Some("ffcam");
                }
            });
            grid(ui, "src", |ui| {
                ui.label("Mirror");
                ui.checkbox(&mut p.mirror, "");
                ui.end_row();
                ui.label("遮罩來源");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut p.matte_source, MatteSource::Ai, "AI");
                    ui.selectable_value(&mut p.matte_source, MatteSource::Luma, "亮度");
                    ui.selectable_value(&mut p.matte_source, MatteSource::Full, "全畫面");
                });
                ui.end_row();
                ui.label("監看小窗");
                ui.checkbox(&mut p.monitors, "");
                ui.end_row();
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("模型：{}", model_name.as_deref().unwrap_or("未載入"))).small().color(MUTED));
                if ui.small_button("選擇模型…").clicked() {
                    action = Some("model");
                }
            });
            ui.collapsing("記憶緩衝設定", |ui| {
                grid(ui, "buf", |ui| {
                    ui.label("緩衝長邊");
                    ui.horizontal(|ui| {
                        for r in [480, 640, 960] {
                            ui.selectable_value(&mut p.buffer_res, r, r.to_string());
                        }
                    });
                    ui.end_row();
                    slider(ui, "記憶取樣率", &mut p.capture_rate, 10.0..=30.0, 1.0, " fps");
                    slider(ui, "記憶長度", &mut p.memory_length, 2.0..=16.0, 0.5, " s");
                });
                let cap = crate::memory::capacity_for(p, 256);
                ui.label(RichText::new(format!("可存 {cap} 張 ≈ {:.1} s；改完按 Reset memory", cap as f32 / p.capture_rate)).small().color(MUTED));
            });
        });
        match action {
            Some("camera") => self.open_camera(),
            Some("ffcam") => self.open_ffmpeg_camera(),
            Some("video") => {
                if let Some(path) = rfd::FileDialog::new().add_filter("影片", &["mp4", "mov", "m4v", "webm", "mkv", "avi"]).pick_file() {
                    self.open_video(&path);
                }
            }
            Some("pause") => self.toggle_pause(),
            Some("stop") => {
                self.source = None;
                self.message = "已停止來源，緩衝保留".into();
            }
            Some("reset") => {
                self.engine.reset_memory(&self.p);
                self.seg.reset();
            }
            Some("list") => {
                #[cfg(feature = "camera")]
                {
                    self.cameras = source::list_cameras();
                    if self.cameras.is_empty() {
                        self.message = "沒有找到攝影機".into();
                    }
                }
            }
            Some("model") => {
                if let Some(path) = rfd::FileDialog::new().add_filter("ONNX 模型", &["onnx"]).pick_file() {
                    self.seg.load_model(path);
                    self.p.matte_source = MatteSource::Ai;
                }
            }
            _ => {}
        }
    }

    fn beat_section(&mut self, ui: &mut egui::Ui) {
        let now = self.now();
        let p = &mut self.p;
        let beat = &mut self.engine.beat;
        section(ui, "節拍同步", false, |ui| {
            grid(ui, "beat", |ui| {
                ui.label("BPM");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.bpm).range(60.0..=200.0).speed(0.1).fixed_decimals(1));
                    if ui.small_button("×2").clicked() {
                        p.bpm = (p.bpm * 2.0).min(200.0);
                    }
                    if ui.small_button("÷2").clicked() {
                        p.bpm = (p.bpm / 2.0).max(60.0);
                    }
                    if ui.button("Tap").clicked() {
                        if let Some(b) = beat.tap(now) {
                            p.bpm = (b * 10.0).round() / 10.0;
                        }
                    }
                    let (_, phase) = beat.phase(now, p.bpm);
                    let c = if phase < 0.15 { RED } else { HAIRLINE };
                    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 5.0, c);
                });
                ui.end_row();
                ui.label("Beat sync");
                ui.checkbox(&mut p.beat_sync, "用 BPM 決定間隔");
                ui.end_row();
                ui.label("間隔拍數");
                ui.horizontal(|ui| {
                    for (v, name) in [(0.125, "1/8"), (0.25, "1/4"), (0.5, "1/2"), (1.0, "1"), (2.0, "2")] {
                        ui.selectable_value(&mut p.beat_division, v, name);
                    }
                });
                ui.end_row();
                slider(ui, "拍點脈衝", &mut p.beat_pulse, 0.0..=100.0, 1.0, " %");
            });
            if ui.button("從現在對第一拍").clicked() {
                beat.restart(now);
            }
            ui.label(RichText::new("原型尚未匯入音樂：BPM 手動輸入或連按 Tap 4 下以上。").small().color(MUTED));
        });
    }

    fn guide_window(&mut self, ui: &mut egui::Ui) {
        let mut open = self.show_guide;
        egui::Window::new("操作說明").open(&mut open).default_width(420.0).show(ui.ctx(), |ui| {
            ui.label("1. 選來源：Start camera（攝影機）或 Load video（影片檔，循環播放）。");
            ui.label("2. 人物被分離後存進記憶緩衝；分身 = 往回取第 i × 間隔 秒的影格。");
            ui.label("3. 02 區調分身數、時間間隔、間距、尺寸、軸心；03–05 調外觀。");
            ui.label("4. Spacing 0 時分身疊在一起，但動作仍有時間差。");
            ui.add_space(6.0);
            ui.label(RichText::new("快捷鍵").strong());
            ui.label("H 隱藏介面 · F 全螢幕 · Esc 離開全螢幕 · R Reset memory · 空白鍵 暫停 · Shift+R 錄影");
        });
        self.show_guide = open;
    }
}

fn section(ui: &mut egui::Ui, title: &str, open: bool, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(2.0);
    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, HAIRLINE));
    egui::CollapsingHeader::new(RichText::new(title).strong().color(INK)).default_open(open).show(ui, add);
}

fn grid(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([10.0, 6.0]).show(ui, add);
}

fn slider(ui: &mut egui::Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, suffix: &str) {
    ui.label(label);
    let decimals = if step >= 1.0 { 0 } else if step >= 0.1 { 1 } else { 2 };
    ui.add(egui::Slider::new(v, range).step_by(step).fixed_decimals(decimals).suffix(suffix));
    ui.end_row();
}

/// 瑞士國際主義：白底面板、近黑文字、單一紅色重點、直角、hairline 分隔。
fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some((path, data)) = find_cjk_font() {
        fonts.font_data.insert("cjk".into(), Arc::new(egui::FontData::from_owned(data)));
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push("cjk".into());
        }
        eprintln!("中文字型：{}", path.display());
    } else {
        eprintln!("找不到中文字型，介面中文會顯示成方框；可設定 TIME_ECHO_FONT=字型檔路徑");
    }
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Light);
    ctx.global_style_mut(|style| {
        let v = &mut style.visuals;
        v.panel_fill = Color32::WHITE;
        v.window_fill = Color32::WHITE;
        v.extreme_bg_color = Color32::from_gray(245);
        v.selection.bg_fill = RED;
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.hyperlink_color = RED;
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::ZERO;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, INK);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, INK);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, INK);
        v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        v.widgets.open.fg_stroke = Stroke::new(1.0, INK);
        v.widgets.inactive.bg_fill = Color32::from_gray(238);
        v.widgets.inactive.weak_bg_fill = Color32::from_gray(238);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, INK);
        v.widgets.active.bg_fill = INK;
        v.slider_trailing_fill = true;
        style.spacing.slider_width = 150.0;
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    });
}

fn find_cjk_font() -> Option<(PathBuf, Vec<u8>)> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("TIME_ECHO_FONT").map(PathBuf::from).into_iter().collect();
    candidates.extend(
        [
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/Library/Fonts/Arial Unicode.ttf",
            "/System/Library/Fonts/STHeiti Medium.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ]
        .map(PathBuf::from),
    );
    candidates.into_iter().find_map(|p| std::fs::read(&p).ok().map(|d| (p, d)))
}
