// 影格來源：攝影機（getUserMedia）、本機影片檔、內建示範動畫。
// 共同介面：kind、width、height、drawable、ready()、takeFrame(now)、setPaused(bool)、stop()。

/** 來源長邊上限（和桌面版的 MAX_SOURCE_EDGE 相同）；超過時由 main.js 先縮小。 */
export const MAX_SOURCE_EDGE = 1280;

class VideoSource {
    constructor(video, kind, cleanup) {
        this.video = video;
        this.kind = kind;
        this.cleanup = cleanup;
        this.alive = true;
        this.fresh = false;
        this.lastTime = -1;
        // 有 requestVideoFrameCallback 就靠它知道「有新影格」，沒有就比對 currentTime
        this.useCallback = 'requestVideoFrameCallback' in video;
        if (this.useCallback) {
            const onFrame = () => {
                if (!this.alive) return;
                this.fresh = true;
                video.requestVideoFrameCallback(onFrame);
            };
            video.requestVideoFrameCallback(onFrame);
        }
    }

    get drawable() {
        return this.video;
    }
    get width() {
        return this.video.videoWidth;
    }
    get height() {
        return this.video.videoHeight;
    }

    ready() {
        return this.video.readyState >= 2 && this.video.videoWidth > 0;
    }

    /** 距離上次呼叫之後有沒有新影格。 */
    takeFrame() {
        if (this.useCallback) {
            const fresh = this.fresh;
            this.fresh = false;
            return fresh;
        }
        const t = this.video.currentTime;
        const fresh = t !== this.lastTime;
        this.lastTime = t;
        return fresh;
    }

    setPaused(paused) {
        // 攝影機不能暫停，凍結時只是不再送影格
        if (this.kind !== 'file') return;
        if (paused) this.video.pause();
        else this.video.play().catch(() => {});
    }

    stop() {
        this.alive = false;
        this.video.pause();
        this.cleanup();
        this.video.srcObject = null;
        this.video.removeAttribute('src');
        this.video.load();
    }
}

/** 開啟攝影機。`facing` 是 'user'（前鏡頭）或 'environment'（後鏡頭）。 */
export async function openCamera(video, facing) {
    if (!navigator.mediaDevices?.getUserMedia) {
        throw new Error('這個瀏覽器不能用攝影機（需要 HTTPS 或 localhost）');
    }
    const stream = await navigator.mediaDevices.getUserMedia({
        audio: false,
        video: { facingMode: facing, width: { ideal: 1280 }, height: { ideal: 720 } },
    });
    video.srcObject = stream;
    await video.play();
    return new VideoSource(video, 'camera', () => stream.getTracks().forEach((t) => t.stop()));
}

/** 播放使用者選的影片檔（循環、靜音）。 */
export async function openFile(video, file) {
    const url = URL.createObjectURL(file);
    video.srcObject = null;
    video.src = url;
    video.loop = true;
    try {
        await video.play();
    } catch (e) {
        URL.revokeObjectURL(url);
        throw new Error('這個影片格式瀏覽器不能播放');
    }
    return new VideoSource(video, 'file', () => URL.revokeObjectURL(url));
}

/** 內建示範動畫：黑底上一個來回移動的發光人形，不需要攝影機權限，適合亮度遮罩。 */
export class DemoSource {
    constructor() {
        this.kind = 'demo';
        this.canvas = document.createElement('canvas');
        this.canvas.width = 640;
        this.canvas.height = 360;
        this.ctx = this.canvas.getContext('2d');
        this.paused = false;
        this.clock = 0;
        this.last = null;
    }

    get drawable() {
        return this.canvas;
    }
    get width() {
        return this.canvas.width;
    }
    get height() {
        return this.canvas.height;
    }

    ready() {
        return true;
    }

    takeFrame(now) {
        if (this.last !== null && !this.paused) this.clock += Math.min(0.1, now - this.last);
        this.last = now;
        if (this.paused) return false;
        this.draw(this.clock);
        return true;
    }

    draw(t) {
        const { ctx } = this;
        const { width: w, height: h } = this.canvas;
        ctx.fillStyle = '#000';
        ctx.fillRect(0, 0, w, h);
        const x = w * 0.5 + Math.sin(t * 0.9) * w * 0.3;
        const feet = h * 0.94;
        const bob = Math.abs(Math.sin(t * 5.4)) * 10;
        const hip = feet - 120 - bob;
        const shoulder = hip - 90;
        const swing = Math.sin(t * 5.4);
        const grad = ctx.createLinearGradient(0, shoulder - 50, 0, feet);
        grad.addColorStop(0, `hsl(${(t * 40) % 360} 90% 78%)`);
        grad.addColorStop(1, `hsl(${(t * 40 + 80) % 360} 90% 62%)`);
        ctx.strokeStyle = grad;
        ctx.fillStyle = grad;
        ctx.lineCap = 'round';
        const limb = (x0, y0, x1, y1, width) => {
            ctx.lineWidth = width;
            ctx.beginPath();
            ctx.moveTo(x0, y0);
            ctx.lineTo(x1, y1);
            ctx.stroke();
        };
        limb(x, shoulder, x, hip, 34); // 軀幹
        limb(x, hip, x + swing * 46, feet, 20); // 腿
        limb(x, hip, x - swing * 46, feet, 20);
        limb(x, shoulder + 8, x - swing * 62, shoulder + 70 - Math.abs(swing) * 60, 15); // 手
        limb(x, shoulder + 8, x + swing * 62, shoulder + 70 - Math.abs(swing) * 20, 15);
        ctx.beginPath();
        ctx.arc(x + swing * 4, shoulder - 34, 22, 0, Math.PI * 2);
        ctx.fill();
    }

    setPaused(paused) {
        this.paused = paused;
    }

    stop() {}
}
