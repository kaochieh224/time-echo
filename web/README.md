# TIME ECHO 網頁版（手機測試版）

桌面版的獨立改寫：純 JavaScript ＋ WebGPU，沒有建置步驟，`web/` 整個資料夾就是可以直接發佈的靜態網站。

## 執行

```bash
node scripts/web-dev-server.mjs          # http://localhost:8765/
node --test web/tests/core.test.mjs      # 純計算模組的測試
```

攝影機與 WebGPU 都需要安全來源（https 或 localhost）。要用手機實測，得把 `web/` 放到 https 網址上（例如 GitHub Pages）。

需要支援 WebGPU 的瀏覽器：Android 的 Chrome 121 以上、iOS 26 以上的 Safari、桌面的 Chrome／Edge。

## 和桌面版的對應

| 網頁版 | 桌面版 | 說明 |
|---|---|---|
| `js/params.js` | `src/params.rs` | JSON 欄位名稱相同，預設組可以互通 |
| `js/memory.js` | `src/memory.rs` | 環形索引與排分身 |
| `js/matte.js` | `src/key.rs`、`src/matte.rs` | 亮度／色鍵遮罩與遮罩整理，在 CPU 上算 |
| `js/curve.js` | `src/curve.rs`、`src/beat.rs` | 曲線 LUT 與節拍時鐘 |
| `js/renderer.js` | `src/render.rs` | WebGPU 管線，最後一個 pass 直接畫到 canvas |
| `js/engine.js` | `src/engine.rs` | 擷取節奏與算圖 |
| `js/sources.js` | `src/source.rs` | 攝影機、本機影片、示範動畫 |
| `js/main.js` | `src/app.rs` | 手機介面與主迴圈 |
| `shaders/*.wgsl` | `src/shaders/*.wgsl` | **原樣複製**，改了一邊要同步另一邊 |

邏輯有 Rust 與 JavaScript 兩份。改排列、遮罩或參數範圍時兩邊都要改；`tests/core.test.mjs` 的斷言沿用桌面版的單元測試，用來確認兩邊行為一致。

## 和桌面版的差異

- 還沒有 AI 人物去背。遮罩來源只有亮度、色鍵、全畫面；載入桌面預設組時 `ai` 會換成 `luma`、`ai_key` 換成 `key`。
- 解析度選項較低：輸出 720／960／1280，緩衝 320／480／640。
- 記憶緩衝另有 96 MB 的上限（`memory.js` 的 `MEMORY_BUDGET_BYTES`），避免手機瀏覽器把分頁砍掉。
- 套用或匯入預設組時，鏡像、解析度與緩衝設定維持目前的值（這些跟裝置有關）。
- 沒有監看小窗、錄影、音樂匯入與影片透明通道。
