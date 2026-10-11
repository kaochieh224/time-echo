// 純計算模組的測試：node --test web/tests/
// 斷言沿用桌面版的單元測試（src/memory.rs、curve.rs、key.rs、matte.rs、params.rs），確認移植後行為一致。

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { estimateBpm } from '../js/audio.js';
import { BeatClock, buildLut } from '../js/curve.js';
import { buildMask, keyAlpha, refine, resizeMask } from '../js/matte.js';
import { EchoRing, MEMORY_BUDGET_BYTES, bufferSize, capacityFor, planEchoes } from '../js/memory.js';
import { BLOCK_H, BLOCK_W, blockMean, compareOrientation, probeBlocks } from '../js/orientation.js';
import { preferredDelegates } from '../js/segment.js';
import { PRESETS, applyPreset, defaults, effectiveInterval, fromJSON } from '../js/params.js';

const near = (a, b, eps = 1e-4) => assert.ok(Math.abs(a - b) < eps, `${a} ≉ ${b}`);

test('環形緩衝會繞回，歷史不夠時隱藏', () => {
    const r = new EchoRing(4);
    assert.equal(r.layerAt(0), null);
    assert.equal(r.push(), 0);
    assert.equal(r.layerAt(0), 0);
    assert.equal(r.layerAt(1), null);
    for (const expected of [1, 2, 3, 0, 1]) assert.equal(r.push(), expected);
    assert.equal(r.count, 4);
    assert.equal(r.layerAt(0), 1);
    assert.equal(r.layerAt(3), 2);
    assert.equal(r.layerAt(4), null);
});

test('緩衝尺寸是偶數，容量受記憶體預算限制', () => {
    assert.deepEqual(bufferSize(1280, 720, 480), [480, 270]);
    assert.deepEqual(bufferSize(720, 1280, 480), [270, 480]);
    assert.equal(capacityFor(8, 15, 480, 270, 256), 120);
    assert.equal(capacityFor(16, 30, 640, 360, 256), Math.floor(MEMORY_BUDGET_BYTES / (640 * 360 * 4)));
    assert.equal(capacityFor(16, 30, 320, 180, 256), 256);
});

function filledRing(capacity, pushes) {
    const r = new EchoRing(capacity);
    for (let i = 0; i < pushes; i++) r.push();
    return r;
}

test('層疊行進：最新的在軸心，舊的往左排，由舊到新繪製', () => {
    const p = { ...defaults(), echo_count: 3, interval: 0.2, spacing: 100, figure_size: 50, axis_x: 0 };
    const plan = planEchoes(p, filledRing(120, 120), 15, 1000, 500, 1000, 500, 0);
    assert.equal(plan.instances.length, 3);
    assert.deepEqual(plan.instances.map((e) => e.index), [2, 1, 0]);
    const newest = plan.instances[2];
    assert.deepEqual(newest.rect, [250, 250, 750, 500]);
    assert.equal(plan.instances[0].rect[0], 250 - 1000);
    // 第 i 個往回 round(i × 0.2 × 15) = 3i 張
    assert.equal(plan.instances[1].layer, 119 - 3);
    assert.equal(plan.instances[0].layer, 119 - 6);
});

test('歷史不夠與超出容量的分身分開計數', () => {
    const p = { ...defaults(), echo_count: 10, interval: 1 };
    const plan = planEchoes(p, filledRing(60, 20), 15, 960, 540, 640, 360, 0);
    // 偏移 0、15 有影格；30、45 還在等；60 以上超出容量
    assert.equal(plan.instances.length, 2);
    assert.equal(plan.waiting, 2);
    assert.equal(plan.beyondCapacity, 6);
});

test('崩解與淡出由新到舊遞增，節拍同步換算間隔', () => {
    const p = { ...defaults(), echo_count: 5, interval: 0.1, disintegrate: 80, echo_fade: 50 };
    const plan = planEchoes(p, filledRing(120, 120), 15, 960, 540, 640, 360, 0.5);
    const oldest = plan.instances[0];
    const newest = plan.instances.at(-1);
    assert.equal(oldest.erosion, 1);
    assert.equal(newest.erosion, 0);
    near(oldest.opacity, 0.5);
    assert.equal(newest.opacity, 1);
    near(effectiveInterval({ ...defaults(), beat_sync: true, bpm: 126, beat_division: 0.5 }), 0.238, 1e-3);
});

test('中性曲線是恆等，示範曲線抬暗部壓亮部且單調', () => {
    const flat = buildLut(0, 0, 0);
    flat.forEach((v, i) => assert.ok(Math.abs(v - i) <= 1, `${i} → ${v}`));
    const lut = buildLut(43, 0, -25);
    assert.ok(lut[64] > 64 + 20);
    assert.ok(lut[191] < 191 - 10);
    assert.equal(lut[0], 0);
    assert.equal(lut[255], 255);
    for (let i = 1; i < 256; i++) assert.ok(lut[i] >= lut[i - 1], '曲線應單調');
});

test('節拍脈衝衰減，打拍估 BPM', () => {
    const c = new BeatClock();
    assert.equal(c.pulse(0, 120), 1);
    near(c.pulse(0.075, 120), 0.5, 1e-3);
    assert.equal(c.pulse(0.2, 120), 0);
    assert.equal(c.pulse(0.5, 120), 1);
    let bpm = null;
    for (let i = 0; i < 6; i++) bpm = c.tap(10 + (i * 60) / 126);
    near(bpm, 126, 0.01);
    assert.equal(c.tap(100), null);
});

/** 每拍一個衰減的低頻鼓聲 */
function clickTrack(bpm, seconds, rate) {
    const out = new Float32Array(seconds * rate);
    const beat = (60 / bpm) * rate;
    for (let i = 0; i < out.length; i++) {
        const since = (i % beat) / rate;
        out[i] = Math.sin(2 * Math.PI * 60 * since) * Math.exp(-since * 30);
    }
    return out;
}

test('BPM 估計：節拍音軌估得準，太短或無聲回傳 null', () => {
    for (const bpm of [126, 90, 174]) {
        const got = estimateBpm(clickTrack(bpm, 20, 22050), 22050);
        assert.ok(Math.abs(got - bpm) < 1.5, `${bpm} → ${got}`);
    }
    assert.equal(estimateBpm(clickTrack(120, 2, 22050), 22050), null);
    assert.equal(estimateBpm(new Float32Array(22050 * 20), 22050), null);
});

test('色鍵：去掉綠幕、保留膚色；黑幕依亮度', () => {
    const green = [0, 0.69, 0.25];
    assert.ok(keyAlpha(green, green, 1, 0.5) < 0.01);
    assert.ok(keyAlpha([0.85, 0.65, 0.55], green, 1, 0.5) > 0.99);
    const edge = keyAlpha([0.3, 0.55, 0.3], green, 1, 0.5);
    assert.ok(edge > 0.1 && edge < 0.9, String(edge));
    assert.ok(keyAlpha([0.3, 0.55, 0.3], green, 1.5, 0.5) < edge);
    const black = [0, 0, 0];
    assert.ok(keyAlpha([0.02, 0.02, 0.02], black, 1, 0.9) < 0.1);
    assert.ok(keyAlpha([0.8, 0.7, 0.6], black, 1, 0.9) > 0.99);
});

function square() {
    const m = new Float32Array(400);
    for (let y = 5; y < 15; y++) for (let x = 5; x < 15; x++) m[y * 20 + x] = 1;
    return m;
}
const area = (m) => m.reduce((a, b) => a + b, 0);

test('遮罩整理：收縮、擴張與柔化', () => {
    assert.equal(area(refine(square(), 20, 20, -1, 0)), 64);
    assert.equal(area(refine(square(), 20, 20, 2, 0)), 196);
    const soft = refine(square(), 20, 20, 0, 2.5);
    near(area(soft), 100, 0.5);
    assert.ok(soft[10 * 20 + 5] > 0.2 && soft[10 * 20 + 5] < 0.8);
    assert.ok(soft[10 * 20 + 10] > 0.99);
    assert.deepEqual(refine(square(), 20, 20, 0, 0), square());
});

test('buildMask：全畫面不讀畫素，亮度遮罩取 Rec.709 亮度', () => {
    const full = buildMask({ ...defaults(), matte_source: 'full' }, 4, 4, () => assert.fail('不該讀畫素'));
    assert.deepEqual([full.width, full.height, full.data[0]], [1, 1, 255]);
    const p = { ...defaults(), matte_source: 'luma', edge_softness: 0 };
    const rgba = new Uint8ClampedArray([255, 255, 255, 255, 0, 0, 0, 255, 0, 255, 0, 255, 255, 0, 0, 255]);
    const m = buildMask(p, 2, 2, () => rgba);
    assert.deepEqual([...m.data], [255, 0, 182, 54]);
});

test('AI 遮罩：縮放到緩衝解析度、和色鍵相乘，模型沒載入時當成全畫面', () => {
    near(resizeMask(new Float32Array([0, 1]), 2, 1, 4, 1)[1], 0.25);
    const p = { ...defaults(), matte_source: 'ai', edge_softness: 0 };
    const none = buildMask(p, 2, 2, () => assert.fail('不該讀畫素'), null);
    assert.deepEqual([none.width, none.data[0]], [1, 255]);
    // 模型輸出 1×1 的 0.5，放大成 2×2
    const half = { data: new Float32Array([0.5]), width: 1, height: 1 };
    const m = buildMask(p, 2, 2, () => assert.fail('純 AI 不該讀畫素'), () => half);
    assert.deepEqual([...m.data], [128, 128, 128, 128]);
    // 尺寸相同時也不能改到模型的輸出
    const same = { data: new Float32Array([1, 1, 1, 1]), width: 2, height: 2 };
    buildMask({ ...p, shrink_grow: -1 }, 2, 2, () => null, () => same);
    assert.deepEqual([...same.data], [1, 1, 1, 1]);
    // AI × 色鍵：綠幕的畫素被色鍵去掉，膚色保留 AI 的值
    const green = [0, 176, 64, 255];
    const skin = [217, 166, 140, 255];
    const rgba = new Uint8ClampedArray([...green, ...skin, ...skin, ...green]);
    const both = buildMask({ ...p, matte_source: 'ai_key', screen_colour: '#00B040' }, 2, 2, () => rgba, () => half);
    assert.deepEqual([...both.data], [0, 128, 128, 0]);
});

test('來源方向檢查：分得出正的、上下顛倒與分不出來', () => {
    assert.equal(probeBlocks(100, 100), null);
    assert.deepEqual(probeBlocks(640, 360), [[[0, 0], [288, 0], [576, 0]], [[0, 344], [288, 344], [576, 344]]]);
    const block = new Uint8Array(BLOCK_W * BLOCK_H * 4).fill(40);
    assert.deepEqual(blockMean(block), [40, 40, 40]);
    const sky = [[90, 140, 220], [100, 150, 230], [80, 130, 210]];
    const floor = [[60, 40, 30], [70, 50, 35], [50, 35, 25]];
    const jitter = (rows) => rows.map((c) => c.map((v) => v + 3));
    assert.equal(compareOrientation([jitter(sky), jitter(floor)], [sky, floor]), 'ok');
    assert.equal(compareOrientation([jitter(floor), jitter(sky)], [sky, floor]), 'flipped');
    // 上下緣幾乎一樣（例如全黑）時不下結論
    assert.equal(compareOrientation([floor, floor], [floor, jitter(floor)]), 'unsure');
    // GPU 的內容和參考畫面對不起來時也不下結論
    const grey = [[128, 128, 128], [128, 128, 128], [128, 128, 128]];
    assert.equal(compareOrientation([grey, grey], [sky, floor]), 'unsure');
});

test('AI 分割的執行方式：WebKit 優先用 CPU，Blink 優先用 GPU', () => {
    const chrome = 'Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36';
    const edge = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0';
    const safari = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15';
    // iPad 上的 Chrome 也是 WebKit，UA 裡是 CriOS 而不是 Chrome/
    const chromeOnIpad = 'Mozilla/5.0 (iPad; CPU OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/141.0.0.0 Mobile/15E148 Safari/604.1';
    assert.deepEqual(preferredDelegates(chrome), ['GPU', 'CPU']);
    assert.deepEqual(preferredDelegates(edge), ['GPU', 'CPU']);
    assert.deepEqual(preferredDelegates(safari), ['CPU', 'GPU']);
    assert.deepEqual(preferredDelegates(chromeOnIpad), ['CPU', 'GPU']);
});

test('參數：補預設值、夾範圍、桌面版預設組可載入', () => {
    const p = fromJSON('{"echo_count": 999, "clip_black": 80, "clip_white": 20, "bogus": 1, "spacing": "x"}');
    assert.equal(p.echo_count, 48);
    assert.equal(p.clip_black, 80);
    assert.equal(p.clip_white, 81);
    assert.equal(p.spacing, defaults().spacing);
    assert.ok(!('bogus' in p));
    // 桌面版的欄位：解析度取最接近的選項，顏色正規化成大寫，不認得的遮罩來源退回預設
    const desktop = fromJSON('{"matte_source": "ai_key", "output_res": 1920, "buffer_res": 960, "bg_colour": "#ff000c", "video_alpha": true}');
    assert.deepEqual([desktop.matte_source, desktop.output_res, desktop.buffer_res, desktop.bg_colour], ['ai_key', 1280, 640, '#FF000C']);
    assert.equal(fromJSON('{"matte_source": "bogus"}').matte_source, 'ai');
});

test('套用預設組會保留裝置相關的欄位', () => {
    const current = { ...defaults(), mirror: false, matte_source: 'luma', output_res: 720, buffer_res: 320, echo_count: 40 };
    const demo = applyPreset(current, PRESETS.find((x) => x.name === '示範').values);
    assert.deepEqual([demo.mirror, demo.matte_source, demo.output_res, demo.buffer_res], [false, 'luma', 720, 320]);
    assert.deepEqual([demo.echo_count, demo.spacing, demo.bg_colour, demo.clip_white], [22, 241, '#FF000C', 65]);
    assert.deepEqual(applyPreset(defaults(), {}), defaults());
});
