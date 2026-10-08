TIME ECHO：時間分身特效控制器（Windows x64 版）
==============================================

開啟
----
1. 整個 zip 解壓縮到一個資料夾（不要在 zip 裡直接執行）。
2. 雙擊 time-echo.exe。
   第一次開啟時 Windows 可能顯示「Windows 已保護您的電腦」：這版沒有數位簽章，
   按「其他資訊」→「仍要執行」即可。

AI 人物分割
-----------
第一次用 AI 去背時，按 01 LIVE INPUT 欄的「下載 AI 模型」（約 15 MB，需要網路），
下載完自動載入。無法上網的話，執行 download-model.bat，或手動下載
https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx
放進 models 資料夾。沒有模型時仍可用 Keylight 或 Luma 遮罩。

攝影機
------
按 Start camera。若 Windows 詢問相機權限請允許；被擋過的話到
「設定 → 隱私權與安全性 → 相機」開啟「讓桌面應用程式存取相機」。
Start camera 打不開時，可改用 01 欄的「備援攝影機」：填 0（第一台）或裝置名稱，再按 ffmpeg。

錄影
----
REC 或 Shift+R，影片存到「影片」資料夾（time-echo-日期-時間Z.mp4，時間為 UTC）。

快捷鍵
------
H 隱藏介面　F 全螢幕　R Reset memory　空白鍵 暫停　Shift+R 錄影

離線算圖（命令提示字元）
----------------------
time-echo.exe render -i 輸入.mp4 -o 輸出.mp4 --demo

資料夾內容
----------
time-echo.exe          主程式
onnxruntime.dll        ONNX Runtime（AI 分割）
msvcp140.dll 等        Visual C++ runtime
ffmpeg\                影片解碼、錄影（ffmpeg GPL 版）
models\                AI 模型放這裡
presets\               預設組 JSON（可在介面匯入）
licenses\              第三方授權

授權
----
AI 模型 Robust Video Matting 為 GPL-3.0；ffmpeg 為 GPL 版。僅供教學與原型使用。
原始碼：https://github.com/kaochieh224/time-echo
