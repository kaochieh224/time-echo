// CPU 遮罩：亮度、色鍵（對應 src/key.rs）與遮罩整理（對應 src/matte.rs）。
// 輸入是已經縮到緩衝解析度的 RGBA8（ImageData），輸出是 0–1 的 Float32Array。

import { rgb } from './params.js';

/** 幕色是否接近中性色（飽和度低） */
export function isNeutral(screen) {
    return Math.max(...screen) - Math.min(...screen) < 0.15;
}

const luma = (r, g, b) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
const clamp01 = (v) => Math.min(1, Math.max(0, v));

/** 單一畫素的 alpha（1 = 保留前景）。`gain`、`balance` 為 0–2、0–1。 */
export function keyAlpha(c, screen, gain, balance) {
    if (isNeutral(screen)) {
        const dl = Math.abs(luma(c[0], c[1], c[2]) - luma(screen[0], screen[1], screen[2]));
        const dc = Math.hypot(c[0] - screen[0], c[1] - screen[1], c[2] - screen[2]) / Math.sqrt(3);
        const d = balance * dl + (1 - balance) * dc;
        // gain 100 % 時，距離 0.25 以上完全保留
        return clamp01(d * 4 * gain);
    }
    // 幕色主通道 p，其餘 q、r（同值時取後面的，和桌面版的 max_by 一致）
    let p = 0;
    for (let i = 1; i < 3; i++) if (screen[i] >= screen[p]) p = i;
    const q = (p + 1) % 3;
    const r = (p + 2) % 3;
    const sv = (x) => x[p] - (balance * x[q] + (1 - balance) * x[r]);
    const ratio = Math.max(0, sv(c) / Math.max(0.05, sv(screen)));
    return clamp01(1 - ratio * gain);
}

export function lumaMask(rgba, w, h) {
    const m = new Float32Array(w * h);
    for (let i = 0, j = 0; i < m.length; i++, j += 4) {
        m[i] = luma(rgba[j], rgba[j + 1], rgba[j + 2]) / 255;
    }
    return m;
}

export function keyMask(rgba, w, h, screen, gain, balance) {
    const m = new Float32Array(w * h);
    const c = [0, 0, 0];
    for (let i = 0, j = 0; i < m.length; i++, j += 4) {
        c[0] = rgba[j] / 255;
        c[1] = rgba[j + 1] / 255;
        c[2] = rgba[j + 2] / 255;
        m[i] = keyAlpha(c, screen, gain, balance);
    }
    return m;
}

/** 方形結構元素的 min（侵蝕）或 max（擴張），可分離成水平、垂直兩次。 */
function morph(mask, w, h, radius, grow) {
    const pick = grow ? Math.max : Math.min;
    const tmp = new Float32Array(w * h);
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            const hi = Math.min(x + radius, w - 1);
            let v = mask[y * w + Math.max(0, x - radius)];
            for (let xx = Math.max(0, x - radius) + 1; xx <= hi; xx++) v = pick(v, mask[y * w + xx]);
            tmp[y * w + x] = v;
        }
    }
    for (let x = 0; x < w; x++) {
        for (let y = 0; y < h; y++) {
            const hi = Math.min(y + radius, h - 1);
            let v = tmp[Math.max(0, y - radius) * w + x];
            for (let yy = Math.max(0, y - radius) + 1; yy <= hi; yy++) v = pick(v, tmp[yy * w + x]);
            mask[y * w + x] = v;
        }
    }
}

/** 可分離高斯模糊，sigma = 半徑 ÷ 2。 */
function blur(mask, w, h, radius) {
    const sigma = Math.max(0.3, radius * 0.5);
    const k = Math.ceil(radius);
    const weights = new Float32Array(2 * k + 1);
    let sum = 0;
    for (let i = -k; i <= k; i++) sum += weights[i + k] = Math.exp(-(i * i) / (2 * sigma * sigma));
    for (let i = 0; i < weights.length; i++) weights[i] /= sum;
    const tmp = new Float32Array(w * h);
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            let acc = 0;
            for (let j = 0; j < weights.length; j++) {
                const xx = Math.min(w - 1, Math.max(0, x + j - k));
                acc += weights[j] * mask[y * w + xx];
            }
            tmp[y * w + x] = acc;
        }
    }
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            let acc = 0;
            for (let j = 0; j < weights.length; j++) {
                const yy = Math.min(h - 1, Math.max(0, y + j - k));
                acc += weights[j] * tmp[yy * w + x];
            }
            mask[y * w + x] = acc;
        }
    }
}

/** `shrinkGrow`：負值侵蝕、正值擴張（px）；`softness`：模糊半徑（px）。就地修改。 */
export function refine(mask, w, h, shrinkGrow, softness) {
    const r = Math.round(shrinkGrow);
    if (r !== 0) morph(mask, w, h, Math.abs(r), r > 0);
    if (softness > 0.05) blur(mask, w, h, softness);
    return mask;
}

export function toU8(mask) {
    const out = new Uint8Array(mask.length);
    for (let i = 0; i < mask.length; i++) out[i] = clamp01(mask[i]) * 255 + 0.5;
    return out;
}

/**
 * 依遮罩來源算出整理好的遮罩（R8，`w × h`）。
 * `readPixels()` 回傳緩衝解析度的 RGBA8；全畫面遮罩不需要畫素，所以用到才呼叫。
 */
export function buildMask(p, w, h, readPixels) {
    if (p.matte_source === 'full') return { data: new Uint8Array([255]), width: 1, height: 1 };
    const rgba = readPixels();
    const mask =
        p.matte_source === 'key'
            ? keyMask(rgba, w, h, rgb(p.screen_colour), p.screen_gain / 100, p.screen_balance / 100)
            : lumaMask(rgba, w, h);
    refine(mask, w, h, p.shrink_grow, p.edge_softness);
    return { data: toU8(mask), width: w, height: h };
}
