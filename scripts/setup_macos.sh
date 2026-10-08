#!/usr/bin/env bash
# 在 macOS 上準備執行環境：Rust、ffmpeg、ONNX Runtime、人物分割模型。
# 用法：在 rust-app 資料夾裡執行  bash scripts/setup_macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v brew >/dev/null; then
  echo "請先安裝 Homebrew：https://brew.sh" >&2
  exit 1
fi

if ! command -v cargo >/dev/null; then
  echo "→ 安裝 Rust（rustup）"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
fi

echo "→ 安裝 ffmpeg 與 onnxruntime"
brew install ffmpeg onnxruntime

MODEL=models/rvm_mobilenetv3_fp32.onnx
if [ ! -f "$MODEL" ]; then
  echo "→ 下載人物分割模型 Robust Video Matting（約 15 MB）"
  mkdir -p models
  curl -L --fail -o "$MODEL" \
    https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx
fi

echo "→ 編譯（第一次約 5–10 分鐘）"
cargo build --release

echo
echo "完成。啟動：  ./target/release/time-echo"
echo "示範預設組：  ./target/release/time-echo --demo 你的影片.mp4"
