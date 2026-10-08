# CLAUDE.md

這份檔案提供 Claude Code（claude.ai/code）在這個儲存庫裡工作時的指引。

## 這是什麼

TIME ECHO：「時間分身」影片特效控制器的 Rust 桌面原型（TIME & MEMORY 風格）。從攝影機或影片來源把人物去背，把最近的影格存進環形緩衝，再把延遲的複本（「分身」）並排、由 GPU 合成。技術：wgpu + egui/eframe 負責繪圖與介面；ffmpeg **子程序**負責影片解碼、MP3 解碼與 H.264 錄影；`ort`（ONNX Runtime，執行時動態載入）搭配 Robust Video Matting 模型做人物分割；nokhwa 接攝影機；rodio 播放音樂。

程式註解、介面字串、錯誤訊息、commit 訊息與 README 都是繁體中文，新增的內容請沿用。README 與註解會引用「規格」的章節編號（例如規格 2.1、3.2）；那份規格文件（`特效控制器_規格與技術選型.md`）不在這個儲存庫裡。

## 指令

```bash
cargo run                                  # 開啟介面（dev 設定：自己的程式 opt-level 1，相依套件 opt-level 3）
cargo run -- video.mp4 --demo              # 載入影片並套示範預設組
cargo run -- --preset presets/demo-time-memory.json
cargo build --release

# 離線算圖（不開視窗）：驗證整條管線最快的方法
cargo run --release -- render -i in.mp4 -o out.mp4 --demo --matte luma --frames 30
#   --matte ai|luma|full   --preset file.json   --model model.onnx   --mirror

cargo test                                 # 單元測試＋GPU 整合測試
cargo test --lib memory::tests::procession_geometry          # 單一單元測試
cargo test --test gpu_pipeline contour_keeps_only_the_edge   # 單一 GPU 測試
cargo test --release --lib --bins          # CI 跑的那組（不含 GPU 測試）

cargo run --bin export-presets             # 重新產生 presets/*.json；改了 params.rs 的預設值後要重跑
cargo run --release --example bpm -- song.mp3   # 單獨估計 BPM
cargo build --release --no-default-features --features seg   # 不編入 nokhwa（攝影機）與 rodio（音樂）
```

使用 edition 2024 與 let-chains，需要夠新的 stable 工具鏈。在 Windows 編譯需要 MSVC 工具鏈與 NASM（nokhwa 的 mozjpeg 要用）。沒有額外的 lint／format 設定，用預設值。

GPU 整合測試（`tests/gpu_pipeline.rs`）在找不到 wgpu adapter 時會直接略過，不會失敗（lavapipe 這類軟體 adapter 也算有）。

## 執行時相依（執行時才找，不是連結時）

- **ffmpeg／ffprobe**：先看 `TIME_ECHO_FFMPEG`，再找 `<執行檔資料夾>/ffmpeg/`、`<執行檔資料夾>/`、Homebrew 路徑，最後才是 PATH（`source::find_tool`）。一律透過 `source::command()` 啟動子程序，Windows 才不會閃出主控台視窗。
- **ONNX Runtime**：`ORT_DYLIB_PATH` 或執行檔旁（`segment::find_ort_library`）。`ort` 版本釘死在 `=2.0.0-rc.13`，並開 `load-dynamic`。
- **模型**：先看 `TIME_ECHO_MODEL`，否則是 `models/rvm_mobilenetv3_fp32.onnx`（`segment::find_model`）；介面上可以直接下載。沒有模型時 AI 遮罩會退回全畫面；`--matte luma` 不需要模型也不需要 ONNX Runtime。
- `TIME_ECHO_FONT` 可以指定介面用的中文字型。

RVM 模型是 GPL-3.0，刻意不放進儲存庫，也不放進 Windows 的 zip。

## 架構

管線：來源影格 → 遮罩 → 擷取進環形緩衝 → 排分身 → 合成 → 調色 → Bloom → 輸出。

- **`params.rs` 是參數的單一來源。** 數值一律以面板單位（%、秒、px、度）存放，和介面與預設組 JSON 一致；換算成 0–1 的工作在 `render`／`engine` 內做。`#[serde(default)]` 讓舊的預設組還能載入；`sanitize()` 在載入時把值夾回規格範圍。新增一個參數要動的地方：`Params`（含 `Default`、`sanitize`）、`app.rs` 的介面、`render.rs` 的 uniform 打包、WGSL，最後重新產生 `presets/`。
- **`engine.rs` 是兩個前端共用的核心**，差別只在時間怎麼推進。`push_frame` 上傳來源與遮罩，並依取樣率把一層寫進記憶用的 texture array；`render` 排分身並畫出一張輸出。暫停就是不呼叫 `push_frame`（分身一起凍結）。緩衝設定（解析度、取樣率、記憶長度）要等 `reset_memory` 才生效：`applied_buffer` 記的是已套用的值，和 `Params` 裡的值分開。
- **`memory.rs` 只有純計算，不碰 GPU**：環形索引（`EchoRing`）、容量上限，以及 `plan_echoes`（把參數轉成一串 `EchoInstance`：矩形、層號、侵蝕量、不透明度，依舊到新的繪製順序排列）。歷史還不夠的分身會隱藏，不會拿最舊的影格頂替。排列與時間相關的邏輯放這裡，這裡有單元測試。
- **`render.rs` 擁有所有 wgpu 狀態**，只依賴 `Device`／`Queue`，所以 eframe 的裝置（介面）和無視窗裝置（`headless::create_device`）共用同一份程式。記憶緩衝是 texture array；鏡像與 Despill 在擷取時就寫進去，Clip、表面、Disintegrate、調色、色彩模式則在 `shaders/composite.wgsl` 逐個分身計算。`shaders/post.wgsl` 放全畫面 pass（擷取、背景、Bloom、最終輸出、監看小窗）；每個 pass 用自己的 uniform 槽（`Slot`），同一次提交裡才不會互相覆蓋。輸出或監看貼圖重建時 `Renderer::generation` 會加一，介面據此重新向 egui 註冊貼圖。
- **遮罩在 CPU 上算**，解析度與緩衝相同：`segment::build_mask` 依遮罩來源（AI／Keylight／AI × Keylight／亮度／全畫面／影片 alpha）產生遮罩，再套 `matte::refine`（Shrink／grow、Edge softness）。介面版在背景執行緒 `SegWorker` 上跑，最新的影格優先，遮罩會晚到、由 `Engine::update_mask` 補上；離線算圖則每張同步呼叫 `build_mask`。`Segmenter` 是產生遮罩的 trait；ONNX 實作包在 `seg` feature 裡。
- **`source.rs`**：`Source` trait（`poll(now) -> (Frame, dt)`）涵蓋影片檔（ffmpeg 管線）、透過 ffmpeg 的攝影機（avfoundation／dshow）與 nokhwa 攝影機（`camera` feature）。解碼後的影格長邊上限是 `MAX_SOURCE_EDGE`。
- **前端**：`app.rs`（egui 介面、所有面板、快捷鍵、錄影、預設組）與 `headless.rs`（`render` 子命令）。`main.rs` 手動解析參數。Windows 的 release 版用 `windows` 子系統（沒有主控台）；帶任何命令列參數時，`attach_console()` 會接回父程序的主控台。

## Windows 打包與 CI

`.github/workflows/windows.yml` 會跑單元測試、以 `+crt-static` 編譯、組出 `time-echo-windows-x64.zip`（執行檔＋`ffmpeg/`＋`onnxruntime.dll`＋VC++ runtime DLL＋`presets/`＋`packaging/windows/` 裡的檔案），再從另一個工作目錄執行打包好的執行檔做煙霧測試：亮度遮罩與 AI 遮罩各算 30 張。每次推到 `main` 都會產生一份 Actions artifact；推 `v*` 標籤則會把 zip 附到該 Release。凡是會改變「執行檔怎麼找到身旁檔案」的修改，都要確保這個煙霧測試還能通過。
