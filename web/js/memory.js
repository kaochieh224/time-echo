// 環形記憶緩衝的索引計算與分身排列：對應桌面版的 src/memory.rs。
// 這裡只有純計算，不碰 GPU；實際影格存在 Renderer 的 texture array，層號由這裡決定。

import { effectiveInterval } from './params.js';

/** 記憶緩衝的位元組預算：手機瀏覽器的分頁用太多記憶體會被系統砍掉。 */
export const MEMORY_BUDGET_BYTES = 96 * 1024 * 1024;

/** 實際配置的張數 = min(長度 × 取樣率, 記憶體預算, GPU 上限)，至少 2。 */
export function capacityFor(memoryLength, captureRate, bufW, bufH, gpuMaxLayers) {
    const wanted = Math.max(1, Math.round(memoryLength * captureRate));
    const budget = Math.floor(MEMORY_BUDGET_BYTES / (bufW * bufH * 4));
    return Math.max(2, Math.min(wanted, budget, gpuMaxLayers));
}

/** 由來源比例與長邊算出緩衝尺寸（偶數）。 */
export function bufferSize(srcW, srcH, longEdge) {
    const w = Math.max(1, srcW);
    const h = Math.max(1, srcH);
    const s = longEdge / Math.max(w, h);
    const even = (v) => Math.max(1, Math.round((v * s) / 2)) * 2;
    return [even(w), even(h)];
}

export class EchoRing {
    constructor(capacity) {
        this.capacity = Math.max(1, capacity);
        this.head = 0; // 最新一張所在的層
        this.count = 0; // 已存張數（≤ capacity）
    }

    reset() {
        this.head = 0;
        this.count = 0;
    }

    /** 推進一格並回傳要寫入的層號。 */
    push() {
        if (this.count > 0) this.head = (this.head + 1) % this.capacity;
        this.count = Math.min(this.count + 1, this.capacity);
        return this.head;
    }

    /** 往回 `offset` 張的層號；歷史不夠時回傳 null（隱藏，不重複最舊的）。 */
    layerAt(offset) {
        if (offset >= this.count || offset >= this.capacity) return null;
        return (this.head + this.capacity - offset) % this.capacity;
    }
}

/** 第 i 個分身往回的張數：round(i × interval × 取樣率)。 */
export function echoOffset(i, interval, captureRate) {
    return Math.max(0, Math.round(i * interval * captureRate));
}

/**
 * 排出所有分身的位置與層號（畫面像素座標，y 向下）。
 *
 * 回傳 `instances`（由舊到新，也就是繪製順序）、`beyondCapacity`（超出記憶長度的分身數）、
 * `waiting`（歷史還不夠而暫時隱藏的分身數）、`intervalTooSmall`（相鄰分身會取到同一張）。
 * `extraErosion` 是節拍脈衝加在崩解上的量（0–1）。
 */
export function planEchoes(p, ring, captureRate, outW, outH, srcW, srcH, extraErosion) {
    const n = Math.max(1, p.echo_count);
    const interval = effectiveInterval(p);
    const plan = { instances: [], beyondCapacity: 0, waiting: 0, intervalTooSmall: interval < 1 / captureRate };

    // 來源等比塞進輸出 × 人物大小，以底部中點為基準。
    const fit = Math.min(outW / srcW, outH / srcH);
    const size = p.figure_size / 100;
    const fw = srcW * fit * size;
    const fh = srcH * fit * size;

    const step = n > 1 ? ((p.spacing / 100) * outW) / (n - 1) : 0;
    const anchor = outW * 0.5 + (p.axis_x / 100) * outW * 0.5;
    const erosionTotal = Math.min(1, Math.max(0, p.disintegrate / 100 + extraErosion));
    const fade = p.echo_fade / 100;

    for (let i = n - 1; i >= 0; i--) {
        const offset = echoOffset(i, interval, captureRate);
        const layer = ring.layerAt(offset);
        if (layer === null) {
            if (offset >= ring.capacity) plan.beyondCapacity++;
            else plan.waiting++;
            continue;
        }
        const t = n > 1 ? i / (n - 1) : 0;
        let cx;
        if (p.layout === 'centered') {
            cx = anchor + ((n - 1) * 0.5 - i) * step;
        } else if (p.layout === 'symmetric') {
            // 間距以整排寬度的一半計，左右各一半
            const k = Math.ceil(i / 2) * 2;
            cx = i % 2 === 1 ? anchor - k * step * 0.5 : anchor + k * step * 0.5;
        } else {
            cx = anchor - i * step;
        }
        plan.instances.push({
            rect: [cx - fw * 0.5, outH - fh, cx + fw * 0.5, outH],
            layer,
            index: i, // 0 = 最新
            erosion: erosionTotal * t,
            opacity: 1 - fade * t,
        });
    }
    return plan;
}
