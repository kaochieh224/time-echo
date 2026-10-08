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

## Windows：下載就能用

到 GitHub 的 [Releases](https://github.com/kaochieh224/time-echo/releases) 下載 `time-echo-windows-x64.zip`，解壓後雙擊 `time-echo.exe`。zip 裡已附 ffmpeg、ONNX Runtime 與 VC++ runtime，不必另外安裝。AI 模型第一次使用時在 01 欄按「下載 AI 模型」。詳細說明在 zip 內的 `README-Windows.txt`（原稿在 `packaging/windows/`）。

這個 zip 由 GitHub Actions（`.github/workflows/windows.yml`）在 Windows 主機上編譯、打包，並實際跑一次離線算圖（Luma 與 AI 各 30 張）確認 ffmpeg、GPU、ONNX Runtime 都能用。每次推到 main 都會產生一份（Actions 頁面的 artifact）；推 `v*` 標籤或在 GitHub 發布 Release 時，zip 會自動附到該 Release。

自己在 Windows 編譯：裝 Rust（MSVC 工具鏈）與 NASM 後 `cargo build --release`，再把 `ffmpeg.exe`、`ffprobe.exe` 放進執行檔旁的 `ffmpeg\`、`onnxruntime.dll` 放在執行檔旁。

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

第一次按 **開啟攝影機** 時，macOS 會詢問是否允許「終端機」使用相機。拒絕過的話到「系統設定 → 隱私權與安全性 → 相機」打開。

### 離線算圖（不開視窗）

```bash
./target/release/time-echo render -i 輸入.mp4 -o 輸出.mp4 --demo
#   --matte ai|luma|full   遮罩來源（預設 ai）
#   --preset 檔案.json      用自己的預設組
#   --frames 90             只算前 90 張
#   --mirror                左右鏡像
```

## 操作

介面文字是繁體中文；版面照參考影片：頂端全域列（錄影、淺色／深色、說明、隱藏介面、全螢幕）與狀態列（來源類型、AI FPS、記憶張數）；左側三個診斷監看（01 原始畫面、02 僅人物／棋盤格底、03 黑白遮罩）；中央合成預覽；下方五欄 01–05。配色預設是參考影片的黑底等寬字，右上角可切換成瑞士國際主義白底。下方控制面板的上緣可以拖曳調整高度，各欄內容超出時可各自捲動。

| 快捷鍵 | 作用 |
|---|---|
| `H` | 隱藏／顯示介面 |
| `F` | 全螢幕（`Esc` 離開） |
| `R` | 重設記憶（只清緩衝，同時套用新的緩衝設定） |
| 空白鍵 | 暫停／繼續來源（分身一起凍結） |
| `Shift+R` | 開始／停止錄影，存到「影片」資料夾 |

## 控制項對照（這版的名稱 ← 參考影片的名稱）

| 區 | 這版的控制項（括號內是參考影片的原名） | 狀態 |
|---|---|---|
| 01 即時輸入（LIVE INPUT） | 開啟攝影機（Start camera）、載入影片（Load video）、預設攝影機選單（Default camera）、暫停影片（Pause video）、停止來源（Stop input）、重設記憶（Reset memory）、鏡像畫面（Mirror camera）、影片含透明通道（MOV / video has alpha）、顯示監看小窗（Segmentation screens）、「AI 人物分割已就緒」（AI person segmentation ready） | 全部 |
| | 遮罩來源（AI 人物分割／色鍵／AI × 色鍵／亮度／全畫面）、記憶緩衝設定（長邊、取樣率、記憶長度） | 自加 |
| 02 編排（CHOREOGRAPHY） | 排列模式選單：層疊行進（Reference / layered procession）、置中排列（Centered chorus）、鏡像對稱（Mirror symmetry）；匯入音樂（Import MP3）、暫停音樂（Pause MP3）、BPM 與曲名、節拍同步（Beat sync）、分身數量（Echo figures）、時間間隔（Temporal interval）、分身間距（Echo spacing）、人物大小（Figure size）、水平位置（Axis X） | 全部 |
| | 自動估 BPM、×2／÷2、打拍、間隔拍數、節拍脈衝、分身淡出 | 自加 |
| 03 表面與光（SURFACE & LIGHT） | 表面選單：影像分身（Textured video echoes）、實心剪影（Delayed solid）、輪廓（Contour）；崩解（Disintegrate）、光暈（Light bloom）、色彩選單：原始色彩／白色／單色／自訂顏色（White / monochrome）、自訂顏色（Custom colour）、背景顏色（Background colour，含色條）、地面陰影（Ground shadows） | 全部 |
| 04 色彩與曲線（COLOUR + CURVE） | 亮度（Brightness）、對比（Contrast）、飽和度（Saturation）、RGB 曲線：暗部／中間調／亮部（Shadows／Midtones／Highlights）、選取顏色（SELECTIVE COLOUR）：取色（Pick Colour）＋色票 | 全部 |
| | 選取顏色的色相範圍／色相位移／選色飽和度／明度、雙色調 | 自加 |
| 05 色鍵與輸出（KEYLIGHT + OUTPUT） | 幕色遮罩（SCREEN MATTE）：幕色（Screen colour，可取色）、幕色增益（Screen gain）、幕色平衡（Screen balance）、黑階裁切（Clip black）、白階裁切（Clip white）、收縮／擴張（Shrink / grow）、邊緣柔化（Edge softness）、去溢色（Despill） | 全部 |
| | 輸出解析度、預設組（預設／示範、匯入匯出 JSON） | 自加 |
| 全域 | 說明（Guide）、隱藏介面（Hide UI）、全螢幕（Fullscreen） | 全部 |
| | 錄影、淺色／深色配色 | 自加 |
| | Code ON | **沒做**：影片看不出作用，不臆測 |

說明書標為「待確認」的行為（選單裡的其他選項、選取顏色取色後改什麼、色鍵與 AI 遮罩怎麼合併、節拍同步同步什麼），這版都是**自訂**的，定義寫在規格檔與程式註解。程式內部的參數名稱與預設組 JSON 的欄位仍是英文（例如 `echo_count`、`clip_black`）。

用法重點：

- **取色**：在 04 按「取色」後點中央預覽（或左側 01 原始畫面），「選取顏色」會自動開啟。05 幕色旁的「取色」是點 01 原始畫面取幕色。
- **色鍵**：01 的「遮罩來源」選「色鍵」或「AI × 色鍵」才生效。黑底素材把幕色設成黑色（示範預設組就是 #000000），綠幕就取綠色。去溢色只在用色鍵時作用。
- **影片含透明通道（MOV）**：勾選後直接用影片的透明通道，略過 AI 與色鍵。ProRes 4444 的 MOV 與 VP9 alpha 的 WebM 可用。
- **匯入音樂**：自動估 BPM 並循環播放，第一拍以曲首為準；估成一半或兩倍時按 ×2／÷2，或連按「打拍」。估計 BPM 也可以單獨跑：`cargo run --release --example bpm -- 歌曲.mp3`。
- **地面陰影**：每個分身的剪影壓扁、往右後方斜躺在腳下並模糊，「陰影濃度」調深淺。

## 程式結構

```
src/
  params.rs        參數表（規格第 3 節的單一來源，含預設與示範預設組、JSON）
  memory.rs        環形緩衝索引、容量計算、分身排列（純計算，有單元測試）
  engine.rs        來源影格 → 擷取 → 排分身 → 算圖
  render.rs        wgpu 管線：擷取、合成、Bloom、監看小窗、讀回
  shaders/         composite.wgsl（分身：Clip → 表面 → Disintegrate → 調色 → 色彩模式）、post.wgsl
  segment.rs       ONNX 分割（RVM／單輸入模型）、遮罩來源組合、背景執行緒
  key.rs           Keylight 色鍵、影片 alpha
  audio.rs         MP3 解碼（ffmpeg）、自動估 BPM、播放（rodio）
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

- `cargo build` 通過，無警告；`cargo test`：單元測試 27 項、GPU 整合測試 8 項全過（容器用 lavapipe 軟體 GPU）。
  GPU 測試用合成影格逐點確認：分身位置與取到的歷史影格正確、暫停凍結、重設記憶後重新出現、Clip、Disintegrate、實心剪影、Contour、Ground shadows、Selective colour、Keylight 去背與 Despill、Duotone、Mirror、Bloom。
- 自動 BPM：合成的 126 BPM 鼓點 MP3 估出 126.0；參考影片本身只有 6 秒又有剪接，估出 83.4（不準，這種短片要用「打拍」）。
- 離線算圖：合成的移動人物影片 150 張，AI 分割（RVM）＋示範預設組跑完，輸出 MP4 抽格檢查，紅底、分身排列、侵蝕都正確。
- 介面：在虛擬螢幕開啟並截圖，面板、監看小窗、中文字型正常。
- macOS：以 `aarch64-apple-darwin` 做交叉型別檢查（`cargo check`），含 nokhwa AVFoundation 與介面程式都通過。**沒有在真的 Mac 上連結執行過**，攝影機與 Metal 效能要在 Mac 上實測。

## 疑難排解

- **「AI 分割無法使用：找不到 ONNX Runtime」**：`brew install onnxruntime`；或設定 `export ORT_DYLIB_PATH=/路徑/libonnxruntime.dylib`。
- **找不到模型**：放在 `models/rvm_mobilenetv3_fp32.onnx`，或設定 `TIME_ECHO_MODEL=/路徑/模型.onnx`。
- **nokhwa 攝影機編不過或開不了**：先用 01 欄的「備援攝影機」（欄位填裝置編號後按 ffmpeg；macOS 上的裝置編號可用 `ffmpeg -f avfoundation -list_devices true -i ""` 列出）。完全不編入 nokhwa：`cargo build --release --no-default-features --features seg`。
- **AI FPS 偏低**：分割在 CPU 跑，緩衝長邊選 480 會快很多；分身畫面本身由 GPU 合成，不受影響。
- **中文顯示成方框**：設定 `TIME_ECHO_FONT=/System/Library/Fonts/STHeiti\ Medium.ttc`（或其他中文字型檔）。

## 分割模型授權

Robust Video Matting 為 GPL-3.0，只供本機教學與原型使用；若要散布含模型的程式，請改用授權相容的模型。
