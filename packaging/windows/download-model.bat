@echo off
chcp 65001 >nul
cd /d "%~dp0"
if not exist models mkdir models
echo 下載 AI 分割模型（約 15 MB）...
curl -L --fail -o models\rvm_mobilenetv3_fp32.onnx https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx
if errorlevel 1 (
  echo 下載失敗，請用瀏覽器開上面的網址，存到 models 資料夾。
) else (
  echo 完成。重新開啟 time-echo.exe 即可使用 AI 分割。
)
pause
