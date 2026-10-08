TIME ECHO：時間分身特效控制器（macOS Apple Silicon 版）
====================================================

安裝
----
1. 把「TIME ECHO」拖進「應用程式」資料夾。
2. 這版沒有 Apple 的開發者簽章與公證，第一次開啟會被 macOS 擋下：
   在「應用程式」裡對 TIME ECHO 按右鍵 →「打開」→ 再按一次「打開」。
   若還是打不開，到「系統設定 → 隱私權與安全性」最下方按「仍要打開」，
   或在終端機執行：
       xattr -dr com.apple.quarantine "/Applications/TIME ECHO.app"

需要 Apple Silicon（M1 以後）的 Mac、macOS 13.3 以上。

AI 人物分割
-----------
第一次用 AI 去背時，按「01 / 即時輸入」欄的「下載 AI 模型」（約 15 MB，需要網路），
下載完自動載入。模型存在
    ~/Library/Application Support/time-echo/models/
無法上網的話，手動下載
https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx
放進上面的資料夾。沒有模型時仍可用色鍵或亮度遮罩。

攝影機
------
按「開啟攝影機」，macOS 詢問是否允許使用相機時請允許；拒絕過的話到
「系統設定 → 隱私權與安全性 → 相機」打開 TIME ECHO。

錄影
----
按「錄影」或 Shift+R，影片存到「影片」資料夾（time-echo-日期-時間Z.mp4，時間為 UTC）。

快捷鍵
------
H 隱藏介面　F 全螢幕　R 重設記憶　空白鍵 暫停　Shift+R 錄影

離線算圖（終端機）
----------------
"/Applications/TIME ECHO.app/Contents/MacOS/time-echo" render -i 輸入.mp4 -o 輸出.mp4 --demo

App 內容
--------
ffmpeg、ffprobe（影片解碼、錄影）與 ONNX Runtime（AI 分割）都已附在 App 裡，不必另外安裝。

授權
----
AI 模型 Robust Video Matting 為 GPL-3.0；ffmpeg 為 GPL 版。僅供教學與原型使用。
原始碼：https://github.com/kaochieh224/time-echo
