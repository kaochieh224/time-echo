// 音樂：BPM 估計（對應桌面版 src/audio.rs 的 estimate_bpm）與解碼。

/** 只分析開頭這麼長（s）：夠估 BPM，也避免整首歌佔太多記憶體。 */
const ANALYSE_SECONDS = 120;
const ANALYSE_RATE = 22050;

/** 由單聲道 PCM 估 BPM（回傳 60–200，取到小數一位）。訊號太短或沒有節奏時回傳 null。 */
export function estimateBpm(mono, rate) {
    const hop = 256;
    const frames = mono.length;
    if (frames < rate * 4) return null;

    // 1. 一階低通（約 150 Hz）→ 每 hop 的對數能量
    const alpha = 1 - Math.exp((-2 * Math.PI * 150) / rate);
    let lp = 0;
    let acc = 0;
    const energy = [];
    for (let i = 0; i < frames; i++) {
        lp += alpha * (mono[i] - lp);
        acc += lp * lp;
        if ((i + 1) % hop === 0) {
            energy.push(Math.log(acc / hop + 1e-9));
            acc = 0;
        }
    }

    // 2. 起音包絡：對數能量的正向差分，平滑後去掉平均
    const raw = new Float32Array(energy.length - 1);
    for (let i = 0; i < raw.length; i++) raw[i] = Math.max(0, energy[i + 1] - energy[i]);
    const onset = Float32Array.from(raw);
    for (let i = 1; i < onset.length - 1; i++) onset[i] = 0.25 * raw[i - 1] + 0.5 * raw[i] + 0.25 * raw[i + 1];
    const mean = onset.reduce((a, b) => a + b, 0) / onset.length;
    for (let i = 0; i < onset.length; i++) onset[i] -= mean;
    const fps = rate / hop;

    // 3. 自相關，在 60–200 BPM 範圍找峰值；對 80–160 稍微加權（常見舞曲速度）
    const minLag = Math.floor((60 * fps) / 200);
    const maxLag = Math.ceil((60 * fps) / 60);
    const n = onset.length;
    if (n <= maxLag * 2) return null;
    const ac = (lag) => {
        let sum = 0;
        for (let i = 0; i < n - lag; i++) sum += onset[i] * onset[i + lag];
        return sum / (n - lag);
    };
    let best = 0;
    let bestLag = 0;
    for (let lag = minLag; lag <= maxLag; lag++) {
        const bpm = (60 * fps) / lag;
        const v = ac(lag) * (bpm >= 80 && bpm <= 160 ? 1 : 0.85);
        if (v > best) {
            best = v;
            bestLag = lag;
        }
    }
    if (bestLag === 0) return null;

    // 4. 半速修正：週期訊號在兩倍 lag 也有同樣高的峰；太慢且一半 lag 也夠強，就取快的
    let l = bestLag;
    while (((60 * fps) / l) * 2 <= 200 && (60 * fps) / l < 90) {
        const half = Math.round(l / 2);
        let peak = -Infinity;
        let peakLag = half;
        for (let k = Math.max(0, half - 1); k <= half + 1; k++) {
            const v = ac(k);
            if (v >= peak) {
                peak = v;
                peakLag = k;
            }
        }
        if (half <= minLag || peak < 0.6 * ac(l)) break;
        l = peakLag;
    }

    // 5. 拋物線內插求小數 lag
    const [a, b, c] = [ac(l - 1), ac(l), ac(l + 1)];
    const denom = a - 2 * b + c;
    const shift = Math.abs(denom) > 1e-12 ? Math.min(0.5, Math.max(-0.5, (0.5 * (a - c)) / denom)) : 0;
    return Math.round(((60 * fps) / (l + shift)) * 10) / 10;
}

/** 解碼音樂檔並估 BPM；解不開或估不出來時回傳 null。 */
export async function analyseFile(file) {
    const Offline = globalThis.OfflineAudioContext ?? globalThis.webkitOfflineAudioContext;
    if (!Offline) return null;
    try {
        // 用低取樣率的離線 context 解碼，省記憶體
        const buffer = await new Offline(1, 1, ANALYSE_RATE).decodeAudioData(await file.arrayBuffer());
        const length = Math.min(buffer.length, ANALYSE_SECONDS * buffer.sampleRate);
        const mono = new Float32Array(length);
        for (let ch = 0; ch < buffer.numberOfChannels; ch++) {
            const data = buffer.getChannelData(ch);
            for (let i = 0; i < length; i++) mono[i] += data[i] / buffer.numberOfChannels;
        }
        return estimateBpm(mono, buffer.sampleRate);
    } catch {
        return null;
    }
}
