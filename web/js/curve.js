// RGB 曲線 LUT 與節拍時鐘：對應桌面版的 src/curve.rs、src/beat.rs。

/** 暗部／中間調／亮部（−100–100）→ 256 格的單調曲線（Fritsch–Carlson）。 */
export function buildLut(shadows, mids, highs) {
    const clamp01 = (v) => Math.min(1, Math.max(0, v));
    const xs = [0, 0.25, 0.5, 0.75, 1];
    const ys = [0, clamp01(0.25 + (shadows / 100) * 0.25), clamp01(0.5 + (mids / 100) * 0.25), clamp01(0.75 + (highs / 100) * 0.25), 1];
    const n = xs.length;
    const d = [];
    for (let i = 0; i < n - 1; i++) d.push((ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]));
    const m = new Array(n).fill(0);
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for (let i = 1; i < n - 1; i++) m[i] = d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) * 0.5;
    for (let i = 0; i < n - 1; i++) {
        if (d[i] === 0) {
            m[i] = 0;
            m[i + 1] = 0;
            continue;
        }
        const a = m[i] / d[i];
        const b = m[i + 1] / d[i];
        const s = a * a + b * b;
        if (s > 9) {
            const t = 3 / Math.sqrt(s);
            m[i] = t * a * d[i];
            m[i + 1] = t * b * d[i];
        }
    }
    const lut = new Uint8Array(256);
    for (let k = 0; k < 256; k++) {
        const x = k / 255;
        const i = Math.min(Math.floor(x / 0.25), n - 2);
        const h = xs[i + 1] - xs[i];
        const t = (x - xs[i]) / h;
        const t2 = t * t;
        const t3 = t2 * t;
        const y = (2 * t3 - 3 * t2 + 1) * ys[i] + (t3 - 2 * t2 + t) * h * m[i] + (-2 * t3 + 3 * t2) * ys[i + 1] + (t3 - t2) * h * m[i + 1];
        lut[k] = clamp01(y) * 255 + 0.5;
    }
    return lut;
}

/** 拍點後脈衝衰減到 0 的時間（s） */
const PULSE_DECAY = 0.15;

export class BeatClock {
    constructor() {
        this.origin = 0;
        this.taps = [];
    }

    /** 拍點時為 1，之後線性衰減到 0。 */
    pulse(now, bpm) {
        const beatLen = 60 / Math.max(1, bpm);
        const since = (((now - this.origin) % beatLen) + beatLen) % beatLen;
        return Math.max(0, 1 - since / PULSE_DECAY);
    }

    /** 打拍：累積 4 下以上回傳估計的 BPM，否則回傳 null。停超過 2 秒就重新算。 */
    tap(now) {
        if (this.taps.length && now - this.taps[this.taps.length - 1] > 2) this.taps = [];
        this.taps.push(now);
        if (this.taps.length > 8) this.taps.shift();
        this.origin = now;
        if (this.taps.length < 4) return null;
        const span = this.taps[this.taps.length - 1] - this.taps[0];
        return Math.min(200, Math.max(60, (60 * (this.taps.length - 1)) / span));
    }
}
