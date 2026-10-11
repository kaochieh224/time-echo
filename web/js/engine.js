// 引擎：把「來源影格＋遮罩 → 擷取 → 排分身 → 算圖」串起來。對應桌面版的 src/engine.rs。

import { BeatClock } from './curve.js';
import { EchoRing, bufferSize, capacityFor, planEchoes } from './memory.js';

const bufferKey = (p) => [p.buffer_res, p.capture_rate, p.memory_length];

export class Engine {
    constructor(renderer, p) {
        this.renderer = renderer;
        this.ring = new EchoRing(2);
        this.beat = new BeatClock();
        /** 距離下次擷取累積的來源時間（s）；null 表示下一張一定擷取 */
        this.captureAcc = null;
        this.srcSize = null;
        /** 已套用的緩衝設定（長邊、取樣率、長度）；改了要等重設記憶才生效 */
        this.appliedBuffer = bufferKey(p);
        this.lastPlan = { instances: [], beyondCapacity: 0, waiting: 0, intervalTooSmall: false };
    }

    /** 緩衝設定改了、需要重設記憶才會生效。 */
    bufferSettingsPending(p) {
        return bufferKey(p).join() !== this.appliedBuffer.join();
    }

    /** 重設記憶：只清緩衝，不重設參數；同時套用新的緩衝設定。 */
    resetMemory(p) {
        this.appliedBuffer = bufferKey(p);
        this.ring.reset();
        this.captureAcc = null;
        if (this.srcSize) this.alloc();
    }

    alloc() {
        const [, rate, length] = this.appliedBuffer;
        const [bw, bh] = this.bufferDims();
        const cap = capacityFor(length, rate, bw, bh, this.renderer.maxLayers);
        if (this.renderer.ensureMemory(bw, bh, cap) || this.ring.capacity !== cap) {
            this.ring = new EchoRing(cap);
        }
    }

    /** 取樣率（已套用的那個） */
    get captureRate() {
        return this.appliedBuffer[1];
    }

    /** 緩衝尺寸（遮罩也用這個解析度） */
    bufferDims() {
        return bufferSize(this.srcSize[0], this.srcSize[1], this.appliedBuffer[0]);
    }

    /**
     * 送進一張新的來源影格。`dt` 是這張距離上一張的時間；凍結時不要呼叫，擷取就會停止、分身一起凍結。
     * `makeMask(bufW, bufH)` 回傳 { data, width, height }，只在這張真的要擷取時才會被呼叫。
     */
    pushFrame(p, drawable, width, height, dt, makeMask) {
        if (!this.srcSize || this.srcSize[0] !== width || this.srcSize[1] !== height) {
            this.srcSize = [width, height];
            this.ring.reset();
            this.alloc();
        }
        this.renderer.uploadSource(drawable, width, height);
        const period = 1 / this.captureRate;
        this.captureAcc = this.captureAcc === null ? period : this.captureAcc + dt;
        if (this.captureAcc >= period) {
            this.captureAcc = Math.min(this.captureAcc - period, period);
            const mask = makeMask(...this.bufferDims());
            this.renderer.uploadMask(mask.data, mask.width, mask.height);
            this.renderer.capture(this.ring.push(), p);
        }
    }

    /** 算一張輸出。`time` 是節拍時鐘用的牆上時間（s）。回傳輸出尺寸。 */
    render(p, time) {
        const [sw, sh] = this.srcSize ?? [16, 9];
        const long = p.output_res;
        const even = (v) => Math.round(v / 2) * 2;
        const [ow, oh] = sw >= sh ? [long, even((long * sh) / sw)] : [even((long * sw) / sh), long];
        const pulse = p.beat_pulse > 0 ? (this.beat.pulse(time, p.bpm) * p.beat_pulse) / 100 : 0;
        this.lastPlan = planEchoes(p, this.ring, this.captureRate, ow, oh, sw, sh, pulse);

        const fit = Math.min(ow / sw, oh / sh);
        const rw = (sw * fit) / ow;
        const rh = (sh * fit) / oh;
        this.renderer.render({
            params: p,
            echoes: this.lastPlan.instances,
            outWidth: ow,
            outHeight: oh,
            sourceRectUv: [0.5 - rw * 0.5, 0.5 - rh * 0.5, 0.5 + rw * 0.5, 0.5 + rh * 0.5],
            time,
        });
        return [ow, oh];
    }
}
