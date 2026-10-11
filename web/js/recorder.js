// 錄影：把輸出 canvas（加上音樂，如果有）錄成影片檔。對應桌面版的 src/record.rs，
// 但用瀏覽器的 MediaRecorder，格式由瀏覽器決定（Safari 是 MP4，Chrome 視版本是 MP4 或 WebM）。

const RECORD_FPS = 30;
const BITRATE = 8_000_000;
const TYPES = ['video/mp4;codecs=avc1.42E01E,mp4a.40.2', 'video/mp4', 'video/webm;codecs=vp9,opus', 'video/webm;codecs=vp8,opus', 'video/webm'];

export function canRecord(canvas) {
    return typeof MediaRecorder !== 'undefined' && typeof canvas.captureStream === 'function';
}

export class Recorder {
    /** `audioTrack` 是要一起錄進去的音軌（沒有就傳 null）。建好就開始錄。 */
    constructor(canvas, audioTrack) {
        // 不直接錄 WebGPU 的 canvas：Safari 這樣錄出來的畫面會上下顛倒。
        // 改成每張輸出複製到一張 2D canvas，錄那一張。尺寸固定為開始錄影時的輸出尺寸。
        this.source = canvas;
        this.copy = document.createElement('canvas');
        this.copy.width = canvas.width;
        this.copy.height = canvas.height;
        this.ctx = this.copy.getContext('2d', { alpha: false });
        this.frame();
        const stream = this.copy.captureStream(RECORD_FPS);
        if (audioTrack) stream.addTrack(audioTrack);
        const mimeType = TYPES.find((t) => MediaRecorder.isTypeSupported(t)) ?? '';
        this.recorder = new MediaRecorder(stream, { mimeType, videoBitsPerSecond: BITRATE });
        this.videoTracks = stream.getVideoTracks();
        this.chunks = [];
        this.started = performance.now();
        this.recorder.addEventListener('dataavailable', (e) => {
            if (e.data.size > 0) this.chunks.push(e.data);
        });
        this.recorder.start(1000);
    }

    /** 把剛畫好的輸出複製進錄影用的 canvas；要在算圖的同一個影格裡呼叫。 */
    frame() {
        this.ctx.drawImage(this.source, 0, 0, this.copy.width, this.copy.height);
    }

    get seconds() {
        return (performance.now() - this.started) / 1000;
    }

    /** 停止並回傳 { file, seconds }。 */
    stop() {
        const seconds = this.seconds;
        return new Promise((resolve, reject) => {
            this.recorder.addEventListener('error', (e) => reject(e.error ?? new Error('錄影失敗')));
            this.recorder.addEventListener('stop', () => {
                for (const t of this.videoTracks) t.stop();
                const type = (this.recorder.mimeType || 'video/webm').split(';')[0];
                const ext = type === 'video/mp4' ? 'mp4' : 'webm';
                const stamp = new Date().toISOString().slice(0, 19).replace(/[-:]/g, '').replace('T', '-');
                const file = new File(this.chunks, `time-echo-${stamp}.${ext}`, { type });
                if (file.size === 0) reject(new Error('沒有錄到任何畫面'));
                else resolve({ file, seconds });
            });
            this.recorder.stop();
        });
    }
}
