//! 桌面控制器介面（eframe／egui），版面照參考影片：
//! 頂端全域列與狀態、左側三個診斷監看、中央合成預覽、下方 01–05 五欄控制。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, RichText, Stroke, TextStyle, TextureId};
use eframe::egui_wgpu;

use crate::audio::Track;
use crate::engine::Engine;
use crate::frame::{Frame, Mask};
use crate::params::{BgMode, ColourMode, Layout, MatteSource, Params, Rgb, Surface};
use crate::record::{self, Recorder};
use crate::render::Renderer;
use crate::segment::{self, SegJob, SegWorker};
use crate::source::{self, Source, VideoSource};

const RECORD_FPS: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Theme {
    /// 參考影片：黑底、等寬字、白色重點
    Reference,
    /// 瑞士國際主義：白底、#171717、單一紅色
    Swiss,
}

#[derive(Clone, Copy)]
struct Palette {
    bg: Color32,
    panel: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    on_accent: Color32,
    hairline: Color32,
    warn: Color32,
    widget: Color32,
}

impl Theme {
    fn palette(self) -> Palette {
        match self {
            Theme::Reference => Palette {
                bg: Color32::from_gray(6),
                panel: Color32::from_gray(10),
                text: Color32::from_gray(222),
                muted: Color32::from_gray(128),
                accent: Color32::from_gray(240),
                on_accent: Color32::BLACK,
                hairline: Color32::from_gray(48),
                warn: Color32::from_rgb(0xff, 0x3b, 0x30),
                widget: Color32::from_gray(30),
            },
            Theme::Swiss => Palette {
                bg: Color32::WHITE,
                panel: Color32::WHITE,
                text: Color32::from_rgb(0x17, 0x17, 0x17),
                muted: Color32::from_gray(112),
                accent: Color32::from_rgb(0xd4, 0x10, 0x1a),
                on_accent: Color32::WHITE,
                hairline: Color32::from_gray(216),
                warn: Color32::from_rgb(0xd4, 0x10, 0x1a),
                widget: Color32::from_gray(238),
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    None,
    /// 04 Selective colour：點預覽或原始畫面
    Selective,
    /// 05 Screen colour：點原始畫面
    Screen,
}

pub struct StartOptions {
    pub video: Option<PathBuf>,
    pub params: Params,
}

pub struct TimeEchoApp {
    p: Params,
    engine: Engine,
    render_state: egui_wgpu::RenderState,
    source: Option<Box<dyn Source>>,
    last_frame: Option<Arc<Frame>>,
    seg: SegWorker,
    start: Instant,
    out_tex: Option<TextureId>,
    mon_tex: Option<[TextureId; 3]>,
    tex_generation: u64,
    hide_ui: bool,
    show_guide: bool,
    theme: Theme,
    recorder: Option<Recorder>,
    rec_next: f64,
    message: String,
    frame_times: VecDeque<f64>,
    cameras: Vec<String>,
    camera_index: u32,
    ffmpeg_camera: String,
    model_name: Option<String>,
    /// 背景下載模型的結果
    model_dl: Option<std::sync::mpsc::Receiver<anyhow::Result<PathBuf>>>,
    track: Option<Track>,
    pick: Pick,
}

pub fn run(opts: StartOptions) -> eframe::Result {
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1680.0, 1000.0]).with_title("TIME ECHO · 時間分身控制器"),
        ..Default::default()
    };
    eframe::run_native("time-echo", native, Box::new(move |cc| Ok(Box::new(TimeEchoApp::new(cc, opts)?))))
}

impl TimeEchoApp {
    fn new(cc: &eframe::CreationContext<'_>, opts: StartOptions) -> anyhow::Result<Self> {
        setup_fonts(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx, Theme::Reference);
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
                        "找不到 AI 分割模型（models/{}）。按 01 的「下載 AI 模型」，或改用色鍵／亮度遮罩。",
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
            last_frame: None,
            seg,
            start: Instant::now(),
            out_tex: None,
            mon_tex: None,
            tex_generation: u64::MAX,
            hide_ui: false,
            show_guide: false,
            theme: Theme::Reference,
            recorder: None,
            rec_next: 0.0,
            message,
            frame_times: VecDeque::new(),
            cameras: Vec::new(),
            camera_index: 0,
            ffmpeg_camera: "0".into(),
            model_name: None,
            model_dl: None,
            track: None,
            pick: Pick::None,
        };
        if let Some(v) = opts.video {
            app.open_video(&v);
        }
        Ok(app)
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// 節拍時鐘：有音樂在播就用曲內位置（第一拍 = 曲首），否則用牆上時間。
    fn beat_time(&self) -> f64 {
        self.track.as_ref().and_then(|t| t.position()).unwrap_or_else(|| self.now())
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
            self.message = "這個版本沒有編入 nokhwa 攝影機，請用 ffmpeg 攝影機".into();
        }
    }

    fn open_ffmpeg_camera(&mut self) {
        match source::FfmpegCamera::open(&self.ffmpeg_camera) {
            Ok(c) => self.set_source(Box::new(c)),
            Err(e) => self.message = format!("無法開啟攝影機：{e:#}"),
        }
    }

    fn import_audio(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("音樂", &["mp3", "wav", "m4a", "aac", "flac", "ogg"]).pick_file() else {
            return;
        };
        match Track::load(&path) {
            Ok(t) => {
                if let Some(b) = t.bpm {
                    self.p.bpm = b.clamp(60.0, 200.0);
                    self.message = format!("已匯入 {}，估計 {b:.1} BPM（不準可按 ×2／÷2 或「打拍」）", t.name);
                } else {
                    self.message = format!("已匯入 {}，估不出 BPM，請用「打拍」", t.name);
                }
                self.track = Some(t);
            }
            Err(e) => self.message = format!("無法匯入音樂：{e:#}"),
        }
    }

    fn toggle_pause(&mut self) {
        if let Some(s) = self.source.as_mut() {
            let p = !s.paused();
            s.set_paused(p);
        }
    }

    fn reset_memory(&mut self) {
        self.engine.reset_memory(&self.p);
        self.seg.reset();
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
        // macOS「影片」= ~/Movies，Windows「影片」= %USERPROFILE%\Videos
        let dir = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Movies"))
            .filter(|d| d.is_dir())
            .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Videos")))
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
            let full = self.p.matte_source == MatteSource::Full && !self.p.video_alpha;
            if !full {
                let (bw, bh) = self.engine.buffer_dims(frame.width, frame.height);
                self.seg.submit(SegJob::from_params(frame.clone(), bw, bh, &self.p));
            }
            let white = full.then(|| Mask::filled(1, 1, 1.0));
            self.engine.push_frame(&self.p, &frame, white.as_ref(), dt);
            self.last_frame = Some(frame);
        }
        if let Some(e) = self.source.as_ref().and_then(|s| s.error()) {
            self.message = format!("來源錯誤：{e}");
        }
        let bt = self.beat_time();
        self.engine.render(&self.p, bt, self.p.monitors);

        // 錄影：以 30 fps 牆上時間取樣輸出，畫面掉幀時重複上一張
        if let Some(rec) = self.recorder.as_mut()
            && now >= self.rec_next
            && let Some(out) = self.engine.renderer.read_output()
        {
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
        let linear = eframe::wgpu::FilterMode::Linear;
        self.out_tex = r.output_view().map(|v| egui_r.register_native_texture(dev, v, linear));
        self.mon_tex = r.monitor_views().map(|vs| vs.map(|v| egui_r.register_native_texture(dev, v, linear)));
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
            toggle_fullscreen(ui);
        }
        if esc {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            self.hide_ui = false;
            self.pick = Pick::None;
        }
        if r && shift {
            self.toggle_record();
        } else if r {
            self.reset_memory();
        }
        if space {
            self.toggle_pause();
        }
    }

    /// 取色：從原始影格（uv）讀顏色。
    fn pick_from_source(&mut self, u: f32, v: f32) {
        let Some(f) = self.last_frame.clone() else { return };
        let u = if self.p.mirror { 1.0 - u } else { u };
        let c = f.sample_rgb(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)).map(|x| (x * 255.0).round() as u8);
        self.apply_pick(Rgb(c));
    }

    /// 取色：從輸出畫面（像素）讀顏色。
    fn pick_from_output(&mut self, x: u32, y: u32) {
        let Some(out) = self.engine.renderer.read_output() else { return };
        let i = ((y.min(out.height - 1) * out.width + x.min(out.width - 1)) * 4) as usize;
        self.apply_pick(Rgb([out.rgba[i], out.rgba[i + 1], out.rgba[i + 2]]));
    }

    fn apply_pick(&mut self, c: Rgb) {
        match self.pick {
            Pick::Selective => {
                self.p.selective_colour = c;
                self.p.selective = true;
            }
            Pick::Screen => self.p.screen_colour = c,
            Pick::None => return,
        }
        self.message = format!("取色 {}", c.to_hex_string());
        self.pick = Pick::None;
    }
}

impl eframe::App for TimeEchoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_keys(ui);
        self.step_pipeline();
        self.sync_textures();
        self.poll_model_download();
        let (status, model, ai_fps) = self.seg.status();
        self.model_name = model;
        let pal = self.theme.palette();

        if !self.hide_ui {
            egui::Panel::top("global").frame(panel_frame(pal)).show(ui, |ui| self.top_bar(ui, pal, ai_fps));
            egui::Panel::bottom("controls")
                .frame(panel_frame(pal))
                .resizable(true)
                .default_size(360.0)
                .show(ui, |ui| self.control_columns(ui, pal, &status));
            if self.p.monitors {
                egui::Panel::left("monitors").frame(panel_frame(pal)).resizable(false).exact_size(210.0).show(ui, |ui| self.monitors(ui, pal));
            }
        }
        egui::CentralPanel::no_frame().show(ui, |ui| self.preview(ui, pal));
        if self.show_guide {
            self.guide_window(ui);
        }
        if self.pick != Pick::None {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        ui.ctx().request_repaint();
    }

    fn on_exit(&mut self) {
        if let Some(rec) = self.recorder.take() {
            let _ = rec.finish();
        }
    }
}

// ───────────── 版面 ─────────────

impl TimeEchoApp {
    fn top_bar(&mut self, ui: &mut egui::Ui, pal: Palette, ai_fps: f32) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("TIME ECHO").strong().color(pal.text));
            ui.label(RichText::new("時間分身控制器").color(pal.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("全螢幕").clicked() {
                    toggle_fullscreen(ui);
                }
                if ui.button("隱藏介面").clicked() {
                    self.hide_ui = true;
                }
                if ui.button("說明").clicked() {
                    self.show_guide = !self.show_guide;
                }
                let theme_label = match self.theme {
                    Theme::Reference => "淺色",
                    Theme::Swiss => "深色",
                };
                if ui.button(theme_label).on_hover_text("切換介面配色").clicked() {
                    self.theme = if self.theme == Theme::Reference { Theme::Swiss } else { Theme::Reference };
                    apply_theme(ui.ctx(), self.theme);
                }
                let rec = self.recorder.is_some();
                let btn = egui::Button::new(RichText::new(if rec { "■ 停止錄影" } else { "● 錄影" }).color(if rec { pal.on_accent } else { pal.warn }));
                if ui.add(if rec { btn.fill(pal.warn) } else { btn }).on_hover_text("Shift+R；MP4 存到「影片」資料夾").clicked() {
                    self.toggle_record();
                }
            });
        });
        // 第二列：來源類型（左）、效能狀態（右），對應原介面「LOCAL VIDEO / PERSON ONLY」與「18 AI FPS · 95 ECHO FRAMES」
        ui.horizontal(|ui| {
            let kind = match &self.source {
                None => "無來源",
                Some(s) if s.is_live() => "即時攝影機",
                Some(_) => "本機影片",
            };
            let matte = if self.p.video_alpha {
                "影片透明通道"
            } else {
                match self.p.matte_source {
                    MatteSource::Ai => "僅人物",
                    MatteSource::Key => "色鍵",
                    MatteSource::AiKey => "人物 × 色鍵",
                    MatteSource::Luma => "亮度遮罩",
                    MatteSource::Full => "全畫面",
                }
            };
            let paused = self.source.as_ref().is_some_and(|s| s.paused());
            ui.label(RichText::new(format!("■ {kind} / {matte}{}", if paused { " · 已暫停" } else { "" })).small().color(pal.text));
            if let Some(rec) = &self.recorder {
                ui.label(RichText::new(format!("● 錄影 {:.0} 秒", rec.frames as f32 / RECORD_FPS)).small().color(pal.warn));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ai = if self.p.matte_source == MatteSource::Full || self.p.video_alpha { String::new() } else { format!("AI {ai_fps:.0} FPS · ") };
                ui.label(
                    RichText::new(format!("{ai}記憶 {} 張 · {:.0} FPS", self.engine.ring.count(), self.output_fps())).small().color(pal.text),
                );
            });
        });
    }

    fn preview(&mut self, ui: &mut egui::Ui, pal: Palette) {
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, pal.bg);
        let caption_h = if self.hide_ui { 0.0 } else { 22.0 };
        let rect = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, full.max.y - caption_h)).shrink(if self.hide_ui { 0.0 } else { 8.0 });
        let resp = ui.allocate_rect(rect, egui::Sense::click());
        if let (Some(id), Some((w, h))) = (self.out_tex, self.engine.renderer.output_size()) {
            let s = (rect.width() / w as f32).min(rect.height() / h as f32);
            let img = egui::Rect::from_center_size(rect.center(), egui::vec2(w as f32 * s, h as f32 * s));
            ui.painter().image(id, img, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            if self.pick == Pick::Selective
                && resp.clicked()
                && let Some(pos) = resp.interact_pointer_pos()
                && img.contains(pos)
            {
                let (x, y) = ((pos.x - img.min.x) / s, (pos.y - img.min.y) / s);
                self.pick_from_output(x as u32, y as u32);
            }
        }
        if !self.engine.has_source() && !self.hide_ui {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "開啟攝影機或載入影片", FontId::monospace(16.0), pal.muted);
        }
        if self.pick != Pick::None {
            let hint = match self.pick {
                Pick::Selective => "點預覽或 01 原始畫面取色（Esc 取消）",
                _ => "點左側 01 原始畫面取幕色（Esc 取消）",
            };
            ui.painter().text(rect.left_top() + egui::vec2(10.0, 10.0), egui::Align2::LEFT_TOP, hint, FontId::monospace(13.0), pal.warn);
        }
        if !self.hide_ui {
            let y = full.max.y - caption_h * 0.5;
            ui.painter().text(
                egui::pos2(full.min.x + 8.0, y),
                egui::Align2::LEFT_CENTER,
                "影片＋去背人物＋延遲的實心、輪廓與崩解分身",
                FontId::monospace(11.0),
                pal.muted,
            );
            ui.painter().text(egui::pos2(full.max.x - 8.0, y), egui::Align2::RIGHT_CENTER, "AI 人物遮罩", FontId::monospace(11.0), pal.muted);
        }
    }

    fn monitors(&mut self, ui: &mut egui::Ui, pal: Palette) {
        let (Some(ids), Some((w, h))) = (self.mon_tex, self.engine.renderer.monitor_size()) else {
            ui.label(RichText::new("等待來源").small().color(pal.muted));
            return;
        };
        let width = ui.available_width();
        let names = ["01 / 原始畫面", "02 / 僅人物\n棋盤格底", "03 / 黑白遮罩\n背景＝黑"];
        for (k, (id, name)) in ids.iter().zip(names).enumerate() {
            let resp = ui.add(egui::Image::new((*id, egui::vec2(width, width * h as f32 / w as f32))).sense(egui::Sense::click()));
            if k == 0
                && self.pick != Pick::None
                && resp.clicked()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let r = resp.rect;
                // 監看小窗已經鏡像過，換回原始影格座標
                let u = (pos.x - r.min.x) / r.width();
                let u = if self.p.mirror { 1.0 - u } else { u };
                self.pick_from_source(u, (pos.y - r.min.y) / r.height());
            }
            ui.label(RichText::new(name).small().color(pal.muted));
            ui.add_space(8.0);
        }
    }

    fn control_columns(&mut self, ui: &mut egui::Ui, pal: Palette, status: &str) {
        // 面板高度取自上一張實際用掉的高度；內容只要少用幾 px，面板就會一張一張縮下去，所以先撐滿
        ui.set_min_height(ui.available_height());
        // 訊息列
        let plan = &self.engine.last_plan;
        let mut notes: Vec<String> = Vec::new();
        // 還沒有來源時緩衝尚未配置，容量不具意義
        if plan.beyond_capacity > 0 && self.engine.has_source() {
            notes.push(format!("有 {} 個分身超出記憶長度", plan.beyond_capacity));
        }
        if plan.interval_too_small {
            notes.push("間隔小於取樣間隔，相鄰分身會取到同一張".into());
        }
        if self.engine.buffer_settings_pending(&self.p) {
            notes.push("緩衝設定已改，按「重設記憶」生效".into());
        }
        ui.horizontal(|ui| {
            let line = if !self.message.is_empty() { self.message.clone() } else { status.to_string() };
            ui.label(RichText::new(line).small().color(pal.muted));
            for n in notes {
                ui.label(RichText::new(n).small().color(pal.warn));
            }
        });
        ui.add_space(2.0);
        ui.columns(5, |cols| {
            let titles = ["01 / 即時輸入", "02 / 編排", "03 / 表面與光", "04 / 色彩與曲線", "05 / 色鍵與輸出"];
            for (i, col) in cols.iter_mut().enumerate() {
                col.painter().vline(col.max_rect().left() - 4.0, col.max_rect().y_range(), Stroke::new(1.0, pal.hairline));
                col.label(RichText::new(titles[i]).strong().color(pal.text));
                col.add_space(4.0);
                let height = col.available_height();
                egui::ScrollArea::vertical().id_salt(titles[i]).max_height(height).auto_shrink([false, false]).show(col, |ui| match i {
                    0 => self.col_input(ui, pal),
                    1 => self.col_choreography(ui, pal),
                    2 => self.col_surface(ui, pal),
                    3 => self.col_colour(ui, pal),
                    _ => self.col_keylight(ui, pal),
                });
            }
        });
    }

    fn col_input(&mut self, ui: &mut egui::Ui, pal: Palette) {
        let mut action: Option<&str> = None;
        let paused = self.source.as_ref().is_some_and(|s| s.paused());
        let pending = self.engine.buffer_settings_pending(&self.p);
        ui.horizontal_wrapped(|ui| {
            if ui.button("[ 開啟攝影機 ]").clicked() {
                action = Some("camera");
            }
            if ui.button("[ 載入影片 ]").clicked() {
                action = Some("video");
            }
        });
        ui.horizontal(|ui| {
            let name = self.cameras.get(self.camera_index as usize).cloned().unwrap_or_else(|| format!("預設攝影機 #{}", self.camera_index));
            egui::ComboBox::from_id_salt("cam").selected_text(name).width(ui.available_width() - 50.0).show_ui(ui, |ui| {
                if self.cameras.is_empty() {
                    for i in 0..4 {
                        ui.selectable_value(&mut self.camera_index, i, format!("攝影機 #{i}"));
                    }
                } else {
                    for (i, n) in self.cameras.iter().enumerate() {
                        ui.selectable_value(&mut self.camera_index, i as u32, n);
                    }
                }
            });
            if ui.small_button("列出").clicked() {
                action = Some("list");
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button(if paused { "[ 繼續播放 ]" } else { "[ 暫停影片 ]" }).clicked() {
                action = Some("pause");
            }
            if ui.button("[ 停止來源 ]").clicked() {
                action = Some("stop");
            }
        });
        let reset = egui::Button::new(RichText::new("[ 重設記憶 ]").color(if pending { pal.warn } else { pal.text }));
        if ui.add_sized([ui.available_width(), 20.0], reset).clicked() {
            action = Some("reset");
        }
        let p = &mut self.p;
        ui.checkbox(&mut p.mirror, "鏡像畫面");
        ui.checkbox(&mut p.video_alpha, "影片含透明通道（MOV）");
        ui.checkbox(&mut p.monitors, "顯示監看小窗");
        ui.add_space(4.0);
        ui.label(RichText::new("遮罩來源").small().color(pal.muted));
        egui::ComboBox::from_id_salt("matte")
            .width(ui.available_width())
            .selected_text(matte_name(p.matte_source))
            .show_ui(ui, |ui| {
                for m in [MatteSource::Ai, MatteSource::Key, MatteSource::AiKey, MatteSource::Luma, MatteSource::Full] {
                    ui.selectable_value(&mut p.matte_source, m, matte_name(m));
                }
            });
        let ready = match &self.model_name {
            Some(n) => format!("AI 人物分割已就緒\n{n}"),
            None => "AI 模型未載入".into(),
        };
        ui.label(RichText::new(ready).small().color(pal.muted));
        if self.model_name.is_none() {
            if self.model_dl.is_some() {
                ui.label(RichText::new("下載 AI 模型中（約 15 MB）…").small().color(pal.accent));
            } else if ui.button("[ 下載 AI 模型 ]").clicked() {
                action = Some("download");
            }
        }
        ui.horizontal(|ui| {
            if ui.small_button("選擇模型…").clicked() {
                action = Some("model");
            }
            ui.label(RichText::new("備援攝影機").small().color(pal.muted));
            ui.add(egui::TextEdit::singleline(&mut self.ffmpeg_camera).desired_width(22.0));
            if ui.small_button("ffmpeg").clicked() {
                action = Some("ffcam");
            }
        });
        ui.collapsing(RichText::new("記憶緩衝").small(), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("長邊").small());
                for r in [480, 640, 960] {
                    ui.selectable_value(&mut p.buffer_res, r, r.to_string());
                }
            });
            ref_slider(ui, pal, "取樣率", &mut p.capture_rate, 10.0..=30.0, 1.0, " fps");
            ref_slider(ui, pal, "記憶長度", &mut p.memory_length, 2.0..=16.0, 0.5, " s");
            let cap = crate::memory::capacity_for(p, 256);
            ui.label(RichText::new(format!("{cap} 張 ≈ {:.1} s；按「重設記憶」生效", cap as f32 / p.capture_rate)).small().color(pal.muted));
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
            Some("reset") => self.reset_memory(),
            Some("list") => {
                #[cfg(feature = "camera")]
                {
                    self.cameras = source::list_cameras();
                    if self.cameras.is_empty() {
                        self.message = "沒有找到攝影機".into();
                    }
                }
            }
            Some("download") => {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(segment::download_model());
                });
                self.model_dl = Some(rx);
                self.message = "下載 AI 模型中…".into();
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

    fn poll_model_download(&mut self) {
        let Some(rx) = &self.model_dl else { return };
        match rx.try_recv() {
            Ok(Ok(path)) => {
                self.message = format!("模型已下載：{}", path.display());
                self.seg.load_model(path);
                self.p.matte_source = MatteSource::Ai;
                self.model_dl = None;
            }
            Ok(Err(e)) => {
                self.message = format!("{e:#}");
                self.model_dl = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.model_dl = None,
        }
    }

    fn col_choreography(&mut self, ui: &mut egui::Ui, pal: Palette) {
        let now = self.now();
        let bt = self.beat_time();
        let mut import = false;
        egui::ComboBox::from_id_salt("layout").width(ui.available_width()).selected_text(layout_name(self.p.layout)).show_ui(ui, |ui| {
            for l in [Layout::Procession, Layout::Centered, Layout::Symmetric] {
                ui.selectable_value(&mut self.p.layout, l, layout_name(l));
            }
        });
        ui.horizontal(|ui| {
            if ui.button("[ 匯入音樂 ]").clicked() {
                import = true;
            }
            let playing = self.track.as_ref().is_some_and(|t| !t.paused());
            if ui.add_enabled(self.track.is_some(), egui::Button::new(if playing { "[ 暫停音樂 ]" } else { "[ 播放音樂 ]" })).clicked()
                && let Some(t) = self.track.as_mut()
            {
                t.set_paused(playing);
            }
        });
        let p = &mut self.p;
        let beat = &mut self.engine.beat;
        ui.horizontal(|ui| {
            let (_, phase) = beat.phase(bt, p.bpm);
            let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, if phase < 0.15 { pal.warn } else { pal.hairline });
            let track = self.track.as_ref().map(|t| t.name.as_str()).unwrap_or("未載入音樂");
            ui.label(RichText::new(format!("{:.0} BPM · {track}", p.bpm)).small().color(pal.muted));
        });
        ui.checkbox(&mut p.beat_sync, "節拍同步");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut p.bpm).range(60.0..=200.0).speed(0.1).fixed_decimals(1).suffix(" BPM"));
            if ui.small_button("×2").clicked() {
                p.bpm = (p.bpm * 2.0).min(200.0);
            }
            if ui.small_button("÷2").clicked() {
                p.bpm = (p.bpm / 2.0).max(60.0);
            }
            if ui.small_button("打拍").clicked()
                && let Some(b) = beat.tap(now)
            {
                p.bpm = (b * 10.0).round() / 10.0;
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("拍數").small().color(pal.muted));
            for (v, name) in [(0.125, "1/8"), (0.25, "1/4"), (0.5, "1/2"), (1.0, "1"), (2.0, "2")] {
                ui.selectable_value(&mut p.beat_division, v, name);
            }
        });
        ref_slider(ui, pal, "節拍脈衝", &mut p.beat_pulse, 0.0..=100.0, 1.0, " %");
        ui.add_space(4.0);
        let mut n = p.echo_count as f32;
        ref_slider(ui, pal, "分身數量", &mut n, 1.0..=48.0, 1.0, "");
        p.echo_count = n as u32;
        if p.beat_sync {
            ui.horizontal(|ui| {
                ui.label("時間間隔");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:.3} s ♪", p.effective_interval())).color(pal.warn));
                });
            });
        } else {
            ref_slider(ui, pal, "時間間隔", &mut p.interval, 0.05..=2.0, 0.01, " s");
        }
        ref_slider(ui, pal, "分身間距", &mut p.spacing, -300.0..=300.0, 1.0, " %");
        ref_slider(ui, pal, "人物大小", &mut p.figure_size, 10.0..=300.0, 1.0, " %");
        ref_slider(ui, pal, "水平位置", &mut p.axis_x, -100.0..=100.0, 1.0, "");
        ref_slider(ui, pal, "分身淡出", &mut p.echo_fade, 0.0..=100.0, 1.0, " %");
        if import {
            self.import_audio();
            self.engine.beat.restart(0.0);
        }
    }

    fn col_surface(&mut self, ui: &mut egui::Ui, pal: Palette) {
        let p = &mut self.p;
        egui::ComboBox::from_id_salt("surface").width(ui.available_width()).selected_text(surface_name(p.surface)).show_ui(ui, |ui| {
            for s in [Surface::Textured, Surface::Solid, Surface::Contour] {
                ui.selectable_value(&mut p.surface, s, surface_name(s));
            }
        });
        ref_slider(ui, pal, "崩解", &mut p.disintegrate, 0.0..=100.0, 1.0, " %");
        ref_slider(ui, pal, "光暈", &mut p.bloom, 0.0..=100.0, 1.0, " %");
        egui::ComboBox::from_id_salt("colour_mode").width(ui.available_width()).selected_text(colour_name(p.colour_mode)).show_ui(ui, |ui| {
            for c in [ColourMode::Original, ColourMode::White, ColourMode::Mono, ColourMode::Custom] {
                ui.selectable_value(&mut p.colour_mode, c, colour_name(c));
            }
        });
        colour_row(ui, pal, "自訂顏色", &mut p.custom_colour);
        ui.horizontal(|ui| {
            ui.label(RichText::new("背景").small().color(pal.muted));
            ui.selectable_value(&mut p.bg_mode, BgMode::Solid, "純色");
            ui.selectable_value(&mut p.bg_mode, BgMode::Source, "原始畫面");
        });
        colour_row(ui, pal, "背景顏色", &mut p.bg_colour);
        // 原介面的大色條
        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 14.0), egui::Sense::hover());
        let [cr, cg, cb] = p.bg_colour.0;
        ui.painter().rect_filled(r, 0.0, Color32::from_rgb(cr, cg, cb));
        ui.add_space(4.0);
        ui.checkbox(&mut p.ground_shadows, "地面陰影");
        if p.ground_shadows {
            ref_slider(ui, pal, "陰影濃度", &mut p.shadow_opacity, 0.0..=100.0, 1.0, " %");
        }
    }

    fn col_colour(&mut self, ui: &mut egui::Ui, pal: Palette) {
        let mut pick = false;
        let p = &mut self.p;
        ref_slider(ui, pal, "亮度", &mut p.brightness, -100.0..=100.0, 1.0, " %");
        ref_slider(ui, pal, "對比", &mut p.contrast, 0.0..=200.0, 1.0, " %");
        ref_slider(ui, pal, "飽和度", &mut p.saturation, 0.0..=200.0, 1.0, " %");
        ui.add_space(4.0);
        ui.label(RichText::new("RGB 曲線").small().color(pal.muted));
        ref_slider(ui, pal, "暗部", &mut p.curve_shadows, -100.0..=100.0, 1.0, " %");
        ref_slider(ui, pal, "中間調", &mut p.curve_mids, -100.0..=100.0, 1.0, " %");
        ref_slider(ui, pal, "亮部", &mut p.curve_highs, -100.0..=100.0, 1.0, " %");
        ui.add_space(4.0);
        ui.label(RichText::new("選取顏色").small().color(pal.muted));
        ui.horizontal(|ui| {
            ui.checkbox(&mut p.selective, "");
            let picking = self.pick == Pick::Selective;
            let b = egui::Button::new(if picking { "[ 取色中… ]" } else { "[ 取色 ]" });
            if ui.add(b).clicked() {
                pick = true;
            }
            ui.color_edit_button_srgb(&mut p.selective_colour.0);
        });
        if p.selective {
            ref_slider(ui, pal, "色相範圍", &mut p.selective_tolerance, 5.0..=90.0, 1.0, "°");
            ref_slider(ui, pal, "色相位移", &mut p.selective_hue, -180.0..=180.0, 1.0, "°");
            ref_slider(ui, pal, "選色飽和度", &mut p.selective_sat, -100.0..=100.0, 1.0, " %");
            ref_slider(ui, pal, "明度", &mut p.selective_light, -100.0..=100.0, 1.0, " %");
        }
        ui.add_space(4.0);
        ui.label(RichText::new("雙色調").small().color(pal.muted));
        ui.horizontal(|ui| {
            ui.checkbox(&mut p.duotone, "");
            ui.color_edit_button_srgb(&mut p.duotone_dark.0);
            ui.label(RichText::new("→").color(pal.muted));
            ui.color_edit_button_srgb(&mut p.duotone_light.0);
        });
        if ui.small_button("重設調色").clicked() {
            let d = Params::default();
            (p.brightness, p.contrast, p.saturation) = (d.brightness, d.contrast, d.saturation);
            (p.curve_shadows, p.curve_mids, p.curve_highs) = (0.0, 0.0, 0.0);
            p.duotone = false;
            p.selective = false;
        }
        if pick {
            self.pick = if self.pick == Pick::Selective { Pick::None } else { Pick::Selective };
        }
    }

    fn col_keylight(&mut self, ui: &mut egui::Ui, pal: Palette) {
        let mut pick = false;
        let mut preset: Option<Params> = None;
        let p = &mut self.p;
        ui.label(RichText::new("幕色遮罩").small().color(pal.muted));
        ui.horizontal(|ui| {
            ui.label("幕色");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.color_edit_button_srgb(&mut p.screen_colour.0);
                if ui.small_button(if self.pick == Pick::Screen { "…" } else { "取色" }).clicked() {
                    pick = true;
                }
                ui.label(RichText::new(p.screen_colour.to_hex_string()).small().color(pal.muted));
            });
        });
        ref_slider(ui, pal, "幕色增益", &mut p.screen_gain, 0.0..=200.0, 1.0, " %");
        ref_slider(ui, pal, "幕色平衡", &mut p.screen_balance, 0.0..=100.0, 1.0, " %");
        ref_slider(ui, pal, "黑階裁切", &mut p.clip_black, 0.0..=99.0, 1.0, " %");
        ref_slider(ui, pal, "白階裁切", &mut p.clip_white, 1.0..=100.0, 1.0, " %");
        if p.clip_white < p.clip_black + 1.0 {
            p.clip_white = (p.clip_black + 1.0).min(100.0);
        }
        ref_slider(ui, pal, "收縮／擴張", &mut p.shrink_grow, -10.0..=10.0, 1.0, " px");
        ref_slider(ui, pal, "邊緣柔化", &mut p.edge_softness, 0.0..=10.0, 0.5, " px");
        ref_slider(ui, pal, "去溢色", &mut p.despill, 0.0..=100.0, 1.0, " %");
        if !matches!(p.matte_source, MatteSource::Key | MatteSource::AiKey) {
            ui.label(RichText::new("色鍵要在 01 的「遮罩來源」選「色鍵」或「AI × 色鍵」才生效").small().color(pal.muted));
        }
        ui.add_space(6.0);
        ui.label(RichText::new("輸出解析度").small().color(pal.muted));
        ui.horizontal(|ui| {
            for r in [960, 1280, 1920] {
                ui.selectable_value(&mut p.output_res, r, r.to_string());
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("預設組：預設").clicked() {
                preset = Some(Params { mirror: p.mirror, ..Params::default() });
            }
            if ui.small_button("預設組：示範").clicked() {
                preset = Some(Params { mirror: p.mirror, ..Params::demo() });
            }
            if ui.small_button("匯入 JSON").clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("預設組", &["json"]).pick_file()
            {
                match std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|s| Params::from_json(&s)) {
                    Ok(np) => preset = Some(np),
                    Err(e) => self.message = format!("讀不了預設組：{e:#}"),
                }
            }
            if ui.small_button("匯出 JSON").clicked()
                && let Some(path) = rfd::FileDialog::new().set_file_name("time-echo-preset.json").save_file()
                && let Err(e) = std::fs::write(&path, p.to_json())
            {
                self.message = format!("存檔失敗：{e}");
            }
        });
        if let Some(np) = preset {
            *p = np;
        }
        if pick {
            self.pick = if self.pick == Pick::Screen { Pick::None } else { Pick::Screen };
        }
    }

    fn guide_window(&mut self, ui: &mut egui::Ui) {
        let mut open = self.show_guide;
        egui::Window::new("操作說明").open(&mut open).default_width(460.0).show(ui.ctx(), |ui| {
            ui.label("1. 01：開啟攝影機或載入影片。左側三個監看小窗依序是原始畫面、去背結果、黑白遮罩。");
            ui.label("2. 05：遮罩有破洞或殘留，先調黑階／白階裁切；用色鍵時再調幕色增益／平衡、去溢色。");
            ui.label("3. 02：先定分身數量與時間間隔（時間差），再調分身間距、人物大小、水平位置（空間）。");
            ui.label("4. 03 處理表面與光，04 處理調色。按「取色」後點預覽，只調整選取的顏色。");
            ui.label("5. 匯入音樂會自動估 BPM；勾「節拍同步」後間隔跟著拍子走，「節拍脈衝」讓崩解在拍點跳一下。");
            ui.add_space(6.0);
            ui.label(RichText::new("快捷鍵").strong());
            ui.label("H 隱藏介面 · F 全螢幕 · Esc 離開 · R 重設記憶 · 空白鍵 暫停 · Shift+R 錄影");
        });
        self.show_guide = open;
    }
}

fn toggle_fullscreen(ui: &egui::Ui) {
    let fs = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
}

fn matte_name(m: MatteSource) -> &'static str {
    match m {
        MatteSource::Ai => "AI 人物分割",
        MatteSource::Key => "色鍵",
        MatteSource::AiKey => "AI × 色鍵",
        MatteSource::Luma => "亮度",
        MatteSource::Full => "全畫面（不去背）",
    }
}

fn layout_name(l: Layout) -> &'static str {
    match l {
        Layout::Procession => "層疊行進（參考排列）",
        Layout::Centered => "置中排列",
        Layout::Symmetric => "鏡像對稱",
    }
}

fn surface_name(s: Surface) -> &'static str {
    match s {
        Surface::Textured => "影像分身",
        Surface::Solid => "實心剪影",
        Surface::Contour => "輪廓",
    }
}

fn colour_name(c: ColourMode) -> &'static str {
    match c {
        ColourMode::Original => "原始色彩",
        ColourMode::White => "白色",
        ColourMode::Mono => "單色",
        ColourMode::Custom => "自訂顏色",
    }
}

fn panel_frame(pal: Palette) -> egui::Frame {
    egui::Frame::NONE.fill(pal.panel).inner_margin(egui::Margin::symmetric(10, 6)).stroke(Stroke::new(1.0, pal.hairline))
}

/// 參考影片的滑桿樣式：標籤在左、數值在右，滑桿一整列。
fn ref_slider(ui: &mut egui::Ui, pal: Palette, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, suffix: &str) {
    let decimals = if step >= 1.0 { 0 } else if step >= 0.1 { 1 } else { 2 };
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::DragValue::new(v).range(range.clone()).speed(step).fixed_decimals(decimals).suffix(suffix));
        });
    });
    ui.spacing_mut().slider_width = ui.available_width();
    ui.add(egui::Slider::new(v, range).step_by(step).show_value(false));
    let _ = pal;
}

fn colour_row(ui: &mut egui::Ui, pal: Palette, label: &str, c: &mut Rgb) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.color_edit_button_srgb(&mut c.0);
            ui.label(RichText::new(c.to_hex_string()).small().color(pal.muted));
        });
    });
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let pal = theme.palette();
    ctx.set_theme(if theme == Theme::Reference { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.global_style_mut(|style| {
        // 參考影片用等寬字；瑞士版用無襯線
        let family = if theme == Theme::Reference { FontFamily::Monospace } else { FontFamily::Proportional };
        style.text_styles = [
            (TextStyle::Small, FontId::new(10.5, family.clone())),
            (TextStyle::Body, FontId::new(12.5, family.clone())),
            (TextStyle::Button, FontId::new(12.5, family.clone())),
            (TextStyle::Heading, FontId::new(16.0, family.clone())),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
        let v = &mut style.visuals;
        v.panel_fill = pal.panel;
        v.window_fill = pal.panel;
        v.extreme_bg_color = pal.widget;
        v.faint_bg_color = pal.widget;
        v.selection.bg_fill = pal.accent;
        v.selection.stroke = Stroke::new(1.0, pal.on_accent);
        v.hyperlink_color = pal.accent;
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_stroke = Stroke::new(1.0, pal.hairline);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::ZERO;
            w.fg_stroke = Stroke::new(1.0, pal.text);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal.hairline);
        v.widgets.noninteractive.bg_fill = pal.panel;
        v.widgets.inactive.bg_fill = pal.widget;
        v.widgets.inactive.weak_bg_fill = pal.widget;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, pal.hairline);
        v.widgets.hovered.bg_fill = pal.widget;
        v.widgets.hovered.weak_bg_fill = pal.widget;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, pal.text);
        v.widgets.active.bg_fill = pal.accent;
        v.widgets.active.weak_bg_fill = pal.accent;
        v.widgets.active.fg_stroke = Stroke::new(1.0, pal.on_accent);
        v.slider_trailing_fill = true;
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 2.0);
    });
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some((path, data)) = find_cjk_font() {
        fonts.font_data.insert("cjk".into(), Arc::new(egui::FontData::from_owned(data)));
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push("cjk".into());
        }
        eprintln!("中文字型：{}", path.display());
    } else {
        eprintln!("找不到中文字型，介面中文會顯示成方框；可設定 TIME_ECHO_FONT=字型檔路徑");
    }
    ctx.set_fonts(fonts);
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
            "C:\\Windows\\Fonts\\msjh.ttc",
            "C:\\Windows\\Fonts\\msjh.ttf",
            "C:\\Windows\\Fonts\\mingliu.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ]
        .map(PathBuf::from),
    );
    candidates.into_iter().find_map(|p| std::fs::read(&p).ok().map(|d| (p, d)))
}
