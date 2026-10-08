# TIME ECHO：時間分身特效控制器（Rust 原型）

計畫第 3 項的原型。依《特效控制器_規格與技術選型.md》的參數與功能實作，技術改為 Rust 原生桌面程式：

| 項目 | 選用 |
|---|---|
| 繪圖 | wgpu（macOS 走 Metal），記憶緩衝是 texture array |
| 控制面板 | egui（eframe） |
| 攝影機 | nokhwa（AVFoundation）；備援用 ffmpeg 的 AVFoundation 輸入 |
| 影片檔 | ffmpeg 子程序解碼 |
| 人物分割 | ort（ONNX Runtime）＋ Robust Video Matting 模型 |
| 錄影 | ffmpeg 編碼成 H.264 MP4 |

## macOS 建置與執行

需要：Apple Silicon 或 Intel Mac、Homebrew。

### 一鍵準備

```bash
cd time-memory/rust-app
bash scripts/setup_macos.sh
./target/release/time-echo
```

腳本會做四件事：裝 Rust（沒有的話）、`brew install ffmpeg onnxruntime`、下載分割模型到 `models/`、`cargo build --release`。

### 手動步驟

```bash
# 1. Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# 2. ffmpeg（影片解碼與錄影）＋ ONNX Runtime（AI 分割）
brew install ffmpeg onnxruntime
# 3. 分割模型（約 15 MB）
mkdir -p models
curl -L -o models/rvm_mobilenetv3_fp32.onnx \
  https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx
# 4. 編譯並執行
cargo build --release
./target/release/time-echo
```

啟動參數：

```bash
./target/release/time-echo 影片.mp4              # 直接載入影片
./target/release/time-echo --demo 影片.mp4       # 套「示範 TIME & MEMORY」預設組
./target/release/time-echo --preset presets/demo-time-memory.json
```

第一次按 **Start camera** 時，macOS 會詢問是否允許「終端機」使用相機。拒絕過的話到「系統設定 → 隱私權與安全性 → 相機」打開。

### 離線算圖（不開視窗）

```bash
./target/release/time-echo render -i 輸入.mp4 -o 輸出.mp4 --demo
#   --matte ai|luma|full   遮罩來源（預設 ai）
#   --preset 檔案.json      用自己的預設組
#   --frames 90             只算前 90 張
#   --mirror                左右鏡像
```

## 操作

畫面：中央輸出、左側三個監看小窗（原始畫面／人物＋棋盤格／黑白遮罩）、右側控制面板 01–05 與節拍。頂端狀態列顯示輸出 FPS、AI FPS、緩衝已存張數（對應原介面「18 AI FPS · 95 ECHO FRAMES」）。

| 快捷鍵 | 作用 |
|---|---|
| `H` | 隱藏／顯示介面 |
| `F` | 全螢幕（`Esc` 離開） |
| `R` | Reset memory（只清緩衝，同時套用新的緩衝設定） |
| 空白鍵 | 暫停／繼續來源（分身一起凍結） |
| `Shift+R` | 開始／停止錄影，存到「影片」資料夾 |

遮罩來源三選一：**AI**（人物分割）、**亮度**（黑底舞蹈影片用，再以 Clip black／white 調門檻）、**全畫面**（不去背）。

## 規格對照

已實作（MVP 全部，加上「原型完成」大部分）：

- 01：攝影機、影片（循環、靜音）、Pause、Stop input、Reset memory、Mirror（換來源自動套用預設）、監看小窗、緩衝設定（解析度／取樣率／長度）
- 02：Echo figures、Temporal interval、Echo spacing、Figure size、Axis X、排列模式（行進／置中）、舊影淡出
- 03：表面模式（影像紋理／實心剪影）、Disintegrate、Light bloom、色彩模式（原色／白色／單色／自訂色）、背景（純色／原始畫面）
- 04：Brightness、Contrast、Saturation、三段曲線（256 格 LUT）、Duotone
- 05：Clip black／white、Shrink／grow、Edge softness
- 節拍：BPM、Tap tempo、×2／÷2、Beat sync（間隔＝60 ÷ BPM × 拍數）、拍點脈衝
- 輸出：解析度 960／1280／1920、Hide UI、Fullscreen、錄影、預設組（內建預設／示範，匯入匯出 JSON）

時間模型照規格 2.1：以「已擷取的影格」計時、i = 0 是最新、舊的先畫、歷史不足時隱藏、超出容量時狀態列提示。

與規格不同的地方：

| 規格 | 這版 | 原因 |
|---|---|---|
| 瀏覽器網頁（Vite＋WebGL2） | Rust 原生程式 | 使用者決定改用 Rust |
| MediaPipe selfie 模型 | Robust Video Matting（ONNX） | 全身舞蹈畫面邊緣較好，且有時間穩定性；規格第 9 節列過這個風險。也可在面板「選擇模型…」換成其他單輸入的 ONNX 分割模型 |
| 錄影 WebM | MP4（H.264） | ffmpeg 直接編碼，Mac 上剪輯軟體都吃 |
| Shrink／Softness 在 GPU 擷取 pass | 在分割執行緒（CPU）上做 | 行為相同（只影響之後擷取的影格），程式較簡單、可單元測試 |
| 匯入 MP3 自動估 BPM | 尚未做 | BPM 先手動或 Tap；匯入音樂要另加音訊解碼與播放 |

「白色」「單色」「自訂色」的確切公式是自訂的（見 `src/shaders/composite.wgsl`），原工具的定義沒有取得。第二階段項目（MIDI、色鍵、Selective colour、輪廓模式、透明輸出）沒有做。

## 程式結構

```
src/
  params.rs        參數表（規格第 3 節的單一來源，含預設與示範預設組、JSON）
  memory.rs        環形緩衝索引、容量計算、分身排列（純計算，有單元測試）
  engine.rs        來源影格 → 擷取 → 排分身 → 算圖
  render.rs        wgpu 管線：擷取、合成、Bloom、監看小窗、讀回
  shaders/         composite.wgsl（分身：Clip → 表面 → Disintegrate → 調色 → 色彩模式）、post.wgsl
  segment.rs       ONNX 分割（RVM／單輸入模型）、亮度遮罩、背景執行緒
  matte.rs         Shrink／grow、Edge softness
  curve.rs         三段曲線 LUT
  beat.rs          節拍時鐘、Tap tempo
  source.rs        影片（ffmpeg）、攝影機（nokhwa／ffmpeg）
  record.rs        錄影
  app.rs           egui 介面（瑞士國際主義：白底、#171717、單一紅 #d4101a、直角、hairline）
  headless.rs      離線算圖
presets/           default.json、demo-time-memory.json
scripts/           setup_macos.sh
tests/             gpu_pipeline.rs（GPU 整合測試）
```

## 驗證紀錄（2026-10-08，Linux 雲端容器）

- `cargo build` 通過，無警告；`cargo test`：單元測試 20 項、GPU 整合測試 4 項全過（容器用 lavapipe 軟體 GPU）。
  GPU 測試用「每張一個已知顏色」的合成影格，逐點確認：分身位置與取到的歷史影格正確、暫停凍結、Reset 後重新出現、Clip、Disintegrate、實心剪影、Duotone、Mirror、Bloom。
- 離線算圖：合成的移動人物影片 150 張，AI 分割（RVM）＋示範預設組跑完，輸出 MP4 抽格檢查，紅底、分身排列、侵蝕都正確。
- 介面：在虛擬螢幕開啟並截圖，面板、監看小窗、中文字型正常。
- macOS：以 `aarch64-apple-darwin` 做交叉型別檢查（`cargo check`），含 nokhwa AVFoundation 與介面程式都通過。**沒有在真的 Mac 上連結執行過**，攝影機與 Metal 效能要在 Mac 上實測。

## 疑難排解

- **「AI 分割無法使用：找不到 ONNX Runtime」**：`brew install onnxruntime`；或設定 `export ORT_DYLIB_PATH=/路徑/libonnxruntime.dylib`。
- **找不到模型**：放在 `models/rvm_mobilenetv3_fp32.onnx`，或設定 `TIME_ECHO_MODEL=/路徑/模型.onnx`。
- **nokhwa 攝影機編不過或開不了**：先用面板上的「攝影機（ffmpeg）」（欄位填裝置編號，`ffmpeg -f avfoundation -list_devices true -i ""` 可列出）。完全不編入 nokhwa：`cargo build --release --no-default-features --features seg`。
- **AI FPS 偏低**：分割在 CPU 跑，緩衝長邊選 480 會快很多；分身畫面本身由 GPU 合成，不受影響。
- **中文顯示成方框**：設定 `TIME_ECHO_FONT=/System/Library/Fonts/STHeiti\ Medium.ttc`（或其他中文字型檔）。

## 分割模型授權

Robust Video Matting 為 GPL-3.0，只供本機教學與原型使用；若要散布含模型的程式，請改用授權相容的模型。
