// GPU 管線（WebGPU）：擷取 → 環形記憶緩衝（texture array）→ 分身合成 → Bloom → 畫到 canvas。
// 對應桌面版的 src/render.rs，兩支 WGSL 和桌面版相同（web/shaders 是 src/shaders 的複本）。

import { buildLut } from './curve.js';
import { isNeutral } from './matte.js';
import { BLOCK_H, BLOCK_W, blockMean, compareOrientation, probeBlocks } from './orientation.js';
import { rgb } from './params.js';

const COLOR_FORMAT = 'rgba8unorm';
const BLOOM_FORMAT = 'rgba16float';
const MAX_INSTANCES = 64;
/** Bloom 亮部門檻（規格 3.3：固定 0.7） */
const BLOOM_THRESHOLD = 0.7;
/** Disintegrate 雜訊尺度（規格：先寫死） */
const NOISE_SCALE = 9;
const INSTANCE_BYTES = 32;

// 每個全畫面 pass 用自己的 uniform 槽，同一次提交裡不互相覆蓋。
const SLOT = { capture: 0, background: 1, bright: 2, blurHalfH: 3, blurHalfV: 4, down: 5, blurQuarterH: 6, blurQuarterV: 7, final: 8 };
const SLOT_COUNT = 9;

const TEX = GPUTextureUsage.TEXTURE_BINDING;
const COPY_DST = GPUTextureUsage.COPY_DST;
const ATTACH = GPUTextureUsage.RENDER_ATTACHMENT;
// 來源貼圖：copyExternalImageToTexture 的目標需要 RENDER_ATTACHMENT，方向檢查要讀回所以加 COPY_SRC
const SRC_USAGE = COPY_DST | ATTACH | GPUTextureUsage.COPY_SRC;

/** 來源上傳方式，方向檢查發現顛倒時依序往下換；最後一種逐畫素上傳，方向一定對但比較慢。 */
const SOURCE_MODES = ['direct', 'flip', 'pixels'];
/** 方向檢查最多試幾次（畫面一直分不出上下就放棄，維持目前的方式） */
const MAX_PROBES = 30;

/** 0–1 的色相 */
export function hueOf([r, g, b]) {
    const max = Math.max(r, g, b);
    const d = max - Math.min(r, g, b);
    if (d < 1e-6) return 0;
    let h;
    if (max === r) h = ((((g - b) / d) % 6) + 6) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    return h / 6;
}

export class Renderer {
    /**
     * `context` 是 canvas 的 WebGPU context（已 configure 成 `canvasFormat`）；
     * `shaders` = { post, composite } 兩支 WGSL 原始碼。
     */
    constructor(device, context, canvasFormat, shaders) {
        this.device = device;
        this.context = context;
        this.maxLayers = device.limits.maxTextureArrayLayers;
        this.sampler = device.createSampler({ addressModeU: 'clamp-to-edge', addressModeV: 'clamp-to-edge', magFilter: 'linear', minFilter: 'linear' });

        const texEntry = (binding, viewDimension, sampleType) => ({ binding, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType, viewDimension } });
        const uniformEntry = (binding) => ({ binding, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } });
        const samplerEntry = (binding) => ({ binding, visibility: GPUShaderStage.FRAGMENT, sampler: { type: 'filtering' } });

        this.postLayout = device.createBindGroupLayout({
            entries: [uniformEntry(0), texEntry(1, '2d', 'float'), texEntry(2, '2d', 'float'), texEntry(3, '2d', 'float'), samplerEntry(4)],
        });
        this.compositeLayout = device.createBindGroupLayout({
            entries: [uniformEntry(0), texEntry(1, '2d-array', 'float'), samplerEntry(2), texEntry(3, '2d', 'unfilterable-float')],
        });

        const postShader = device.createShaderModule({ label: 'post.wgsl', code: shaders.post });
        const compShader = device.createShaderModule({ label: 'composite.wgsl', code: shaders.composite });
        this.shaderModules = [postShader, compShader];

        const postPl = device.createPipelineLayout({ bindGroupLayouts: [this.postLayout] });
        const postPipeline = (entryPoint, format) =>
            device.createRenderPipeline({
                label: entryPoint,
                layout: postPl,
                vertex: { module: postShader, entryPoint: 'vs_full' },
                fragment: { module: postShader, entryPoint, targets: [{ format }] },
            });
        this.pCapture = postPipeline('fs_capture', COLOR_FORMAT);
        this.pBackground = postPipeline('fs_background', COLOR_FORMAT);
        this.pBright = postPipeline('fs_bright', BLOOM_FORMAT);
        this.pBlur = postPipeline('fs_blur', BLOOM_FORMAT);
        // 最後一個 pass 直接畫到 canvas
        this.pFinal = postPipeline('fs_final', canvasFormat);

        const compPl = device.createPipelineLayout({ bindGroupLayouts: [this.compositeLayout] });
        const alphaBlend = {
            color: { srcFactor: 'src-alpha', dstFactor: 'one-minus-src-alpha', operation: 'add' },
            alpha: { srcFactor: 'one', dstFactor: 'one-minus-src-alpha', operation: 'add' },
        };
        const compPipeline = (label, vs, fs) =>
            device.createRenderPipeline({
                label,
                layout: compPl,
                vertex: {
                    module: compShader,
                    entryPoint: vs,
                    buffers: [
                        {
                            arrayStride: INSTANCE_BYTES,
                            stepMode: 'instance',
                            attributes: [
                                { shaderLocation: 0, offset: 0, format: 'float32x4' },
                                { shaderLocation: 1, offset: 16, format: 'uint32' },
                                { shaderLocation: 2, offset: 20, format: 'float32' },
                                { shaderLocation: 3, offset: 24, format: 'float32' },
                                { shaderLocation: 4, offset: 28, format: 'float32' },
                            ],
                        },
                    ],
                },
                fragment: { module: compShader, entryPoint: fs, targets: [{ format: COLOR_FORMAT, blend: alphaBlend }] },
            });
        this.pComposite = compPipeline('composite', 'vs_main', 'fs_main');
        this.pShadow = compPipeline('ground shadow', 'vs_shadow', 'fs_shadow');

        const uniform = GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST;
        this.slots = Array.from({ length: SLOT_COUNT }, () => device.createBuffer({ size: 48, usage: uniform }));
        this.globals = device.createBuffer({ size: 144, usage: uniform });
        this.instances = device.createBuffer({ size: INSTANCE_BYTES * MAX_INSTANCES, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });

        this.lut = this.tex(256, 1, 'r8unorm', COPY_DST);
        this.lutKey = '';
        this.src = this.tex(1, 1, COLOR_FORMAT, SRC_USAGE);
        this.sourceMode = 'direct';
        this.sourceSettled = false;
        this.probing = false;
        this.probes = 0;
        // 方向檢查的參考畫面與「逐畫素上傳」共用的 2D canvas
        this.refCanvas = document.createElement('canvas');
        this.refCtx = this.refCanvas.getContext('2d', { willReadFrequently: true });
        this.mask = this.tex(1, 1, 'r8unorm', COPY_DST);
        this.writeTex(this.mask, new Uint8Array([255]), 1);
        this.memory = null;
        this.targets = null;
    }

    /** shader 編譯錯誤（沒有錯誤時回傳空陣列）。 */
    async shaderErrors() {
        const out = [];
        for (const m of this.shaderModules) {
            const info = await m.getCompilationInfo();
            for (const msg of info.messages) {
                if (msg.type === 'error') out.push(`${m.label}:${msg.lineNum} ${msg.message}`);
            }
        }
        return out;
    }

    tex(width, height, format, extraUsage) {
        width = Math.max(1, width);
        height = Math.max(1, height);
        const texture = this.device.createTexture({ size: [width, height], format, usage: TEX | extraUsage });
        return { texture, view: texture.createView(), width, height };
    }

    writeTex(tex, data, bytesPerPx) {
        this.device.queue.writeTexture({ texture: tex.texture }, data, { bytesPerRow: tex.width * bytesPerPx, rowsPerImage: tex.height }, [tex.width, tex.height]);
    }

    /** 上傳來源影格。`drawable` 是 video 或 canvas，`width × height` 是它的實際畫素尺寸。 */
    uploadSource(drawable, width, height) {
        if (this.src.width !== width || this.src.height !== height) {
            this.src.texture.destroy();
            this.src = this.tex(width, height, COLOR_FORMAT, SRC_USAGE);
        }
        if (this.sourceMode === 'pixels') {
            this.drawRef(drawable, width, height);
            this.writeTex(this.src, this.refCtx.getImageData(0, 0, width, height).data, 4);
        } else {
            this.device.queue.copyExternalImageToTexture({ source: drawable, flipY: this.sourceMode === 'flip' }, { texture: this.src.texture }, [width, height]);
        }
    }

    drawRef(drawable, width, height) {
        if (this.refCanvas.width !== width || this.refCanvas.height !== height) {
            this.refCanvas.width = width;
            this.refCanvas.height = height;
        }
        this.refCtx.drawImage(drawable, 0, 0, width, height);
    }

    /** 換了來源：重新檢查方向（不同種類的來源結果可能不一樣）。 */
    resetSourceCheck() {
        this.sourceMode = 'direct';
        this.sourceSettled = false;
        this.probes = 0;
    }

    /**
     * 檢查剛上傳的來源貼圖有沒有上下顛倒，顛倒就換下一種上傳方式。
     * 要在 uploadSource 之後、同一個影格內用同一個 drawable 呼叫；結果確定前可以每張都呼叫。
     * 上傳方式改變時呼叫 `onChange()`（之前擷取的影格方向是錯的，呼叫端要清掉）。
     */
    checkSource(drawable, width, height, onChange) {
        if (this.sourceSettled || this.probing) return;
        const blocks = probeBlocks(width, height);
        if (!blocks || this.sourceMode === 'pixels' || this.src.width !== width || this.src.height !== height) {
            this.sourceSettled = true;
            return;
        }
        this.probing = true;
        const mode = this.sourceMode;
        // 參考畫面：同一張影格用 2D canvas 畫出來
        this.drawRef(drawable, width, height);
        const ref = blocks.map((row) => row.map(([x, y]) => blockMean(this.refCtx.getImageData(x, y, BLOCK_W, BLOCK_H).data)));

        const blockBytes = BLOCK_W * BLOCK_H * 4;
        const flat = blocks.flat();
        const buffer = this.device.createBuffer({ size: blockBytes * flat.length, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
        const enc = this.device.createCommandEncoder();
        flat.forEach(([x, y], i) => {
            enc.copyTextureToBuffer({ texture: this.src.texture, origin: [x, y] }, { buffer, offset: i * blockBytes, bytesPerRow: BLOCK_W * 4 }, [BLOCK_W, BLOCK_H]);
        });
        this.device.queue.submit([enc.finish()]);
        buffer
            .mapAsync(GPUMapMode.READ)
            .then(() => {
                const bytes = new Uint8Array(buffer.getMappedRange());
                const means = flat.map((_, i) => blockMean(bytes, i * blockBytes));
                const result = compareOrientation([means.slice(0, 3), means.slice(3)], ref);
                // 檢查期間來源或上傳方式換過就不採用這次的結果
                if (mode !== this.sourceMode || this.sourceSettled) return;
                this.probes++;
                if (result === 'ok' || this.probes >= MAX_PROBES) {
                    this.sourceSettled = true;
                } else if (result === 'flipped') {
                    this.sourceMode = SOURCE_MODES[SOURCE_MODES.indexOf(mode) + 1];
                    this.probes = 0;
                    onChange?.(this.sourceMode);
                }
            })
            .catch((e) => {
                console.error(e);
                this.sourceSettled = true;
            })
            .finally(() => {
                buffer.destroy();
                this.probing = false;
            });
    }

    /** 上傳遮罩（R8，緊密排列）。 */
    uploadMask(data, width, height) {
        if (this.mask.width !== width || this.mask.height !== height) {
            this.mask.texture.destroy();
            this.mask = this.tex(width, height, 'r8unorm', COPY_DST);
        }
        this.writeTex(this.mask, data, 1);
    }

    /** 配置（或沿用）記憶緩衝；重新配置時回傳 true（內容全清）。 */
    ensureMemory(width, height, layers) {
        layers = Math.min(this.maxLayers, Math.max(2, layers));
        const m = this.memory;
        if (m && m.width === width && m.height === height && m.layerViews.length === layers) return false;
        m?.texture.destroy();
        const texture = this.device.createTexture({ label: 'echo memory', size: [width, height, layers], format: COLOR_FORMAT, usage: TEX | ATTACH });
        const arrayView = texture.createView({ dimension: '2d-array' });
        const layerViews = Array.from({ length: layers }, (_, i) => texture.createView({ dimension: '2d', baseArrayLayer: i, arrayLayerCount: 1 }));
        this.memory = { texture, arrayView, layerViews, width, height };
        return true;
    }

    postBindGroup(slot, a, b, c, t0, t1 = t0, t2 = t0) {
        const buf = this.slots[slot];
        this.device.queue.writeBuffer(buf, 0, new Float32Array([...a, ...b, ...c]));
        return this.device.createBindGroup({
            layout: this.postLayout,
            entries: [
                { binding: 0, resource: { buffer: buf } },
                { binding: 1, resource: t0 },
                { binding: 2, resource: t1 },
                { binding: 3, resource: t2 },
                { binding: 4, resource: this.sampler },
            ],
        });
    }

    fullscreen(enc, pipeline, bindGroup, view) {
        const pass = enc.beginRenderPass({ colorAttachments: [{ view, loadOp: 'clear', clearValue: [0, 0, 0, 1], storeOp: 'store' }] });
        pass.setPipeline(pipeline);
        pass.setBindGroup(0, bindGroup);
        pass.draw(3);
        pass.end();
    }

    /** 把目前的來源影格＋遮罩寫進記憶緩衝第 `layer` 層。 */
    capture(layer, p) {
        if (!this.memory) return;
        const screen = rgb(p.screen_colour);
        // 去溢色只在用到色鍵時生效
        const despill = p.matte_source === 'key' || p.matte_source === 'ai_key' ? p.despill / 100 : 0;
        const bg = this.postBindGroup(SLOT.capture, [...screen, 0], [despill, isNeutral(screen) ? 1 : 0, 0, 0], [p.mirror ? 1 : 0, 0, 0, 0], this.src.view, this.mask.view, this.mask.view);
        const enc = this.device.createCommandEncoder();
        this.fullscreen(enc, this.pCapture, bg, this.memory.layerViews[layer]);
        this.device.queue.submit([enc.finish()]);
    }

    ensureTargets(w, h) {
        const t = this.targets;
        if (t && t.comp.width === w && t.comp.height === h) return;
        if (t) for (const x of Object.values(t)) x.texture.destroy();
        const half = [Math.max(1, w >> 1), Math.max(1, h >> 1)];
        const quarter = [Math.max(1, w >> 2), Math.max(1, h >> 2)];
        this.targets = {
            comp: this.tex(w, h, COLOR_FORMAT, ATTACH),
            halfA: this.tex(...half, BLOOM_FORMAT, ATTACH),
            halfB: this.tex(...half, BLOOM_FORMAT, ATTACH),
            quarterA: this.tex(...quarter, BLOOM_FORMAT, ATTACH),
            quarterB: this.tex(...quarter, BLOOM_FORMAT, ATTACH),
        };
        // canvas 的畫素尺寸就是輸出尺寸，顯示大小交給 CSS
        this.context.canvas.width = w;
        this.context.canvas.height = h;
    }

    /**
     * 畫一張輸出到 canvas。`echoes` 由舊到新；`sourceRectUv` 是背景「原始畫面」模式時
     * 來源在輸出中的矩形（uv：左、上、右、下）。
     */
    render({ params: p, echoes, outWidth, outHeight, sourceRectUv, time }) {
        this.ensureTargets(outWidth, outHeight);
        const queue = this.device.queue;

        const key = [p.curve_shadows, p.curve_mids, p.curve_highs].map(Math.round).join();
        if (this.lutKey !== key) {
            this.writeTex(this.lut, buildLut(p.curve_shadows, p.curve_mids, p.curve_highs), 1);
            this.lutKey = key;
        }

        const mem = this.memory;
        const g = new Float32Array(36);
        g.set([outWidth, outHeight, p.clip_black / 100, p.clip_white / 100], 0);
        g.set([p.brightness / 100, p.contrast / 100, p.saturation / 100, NOISE_SCALE], 4);
        g.set([['textured', 'solid', 'contour'].indexOf(p.surface), ['original', 'white', 'mono', 'custom'].indexOf(p.colour_mode), p.duotone ? 1 : 0, time], 8);
        g.set([...rgb(p.custom_colour), 1], 12);
        g.set([...rgb(p.duotone_dark), 1], 16);
        g.set([...rgb(p.duotone_light), 1], 20);
        g.set([mem ? 1 / mem.width : 0, mem ? 1 / mem.height : 0, p.shadow_opacity / 100, 0], 24);
        g.set([p.selective ? 1 : 0, hueOf(rgb(p.selective_colour)), p.selective_tolerance / 360, p.selective_hue / 360], 28);
        g.set([p.selective_sat / 100, p.selective_light / 100, 0, 0], 32);
        queue.writeBuffer(this.globals, 0, g);

        const count = Math.min(echoes.length, MAX_INSTANCES);
        if (count > 0) {
            const bytes = new ArrayBuffer(count * INSTANCE_BYTES);
            const f = new Float32Array(bytes);
            const u = new Uint32Array(bytes);
            for (let i = 0; i < count; i++) {
                const e = echoes[i];
                f.set(e.rect, i * 8);
                u[i * 8 + 4] = e.layer;
                f[i * 8 + 5] = e.erosion;
                f[i * 8 + 6] = e.opacity;
                f[i * 8 + 7] = e.index;
            }
            queue.writeBuffer(this.instances, 0, bytes);
        }

        const t = this.targets;
        const enc = this.device.createCommandEncoder();

        // 背景
        const bg = this.postBindGroup(SLOT.background, sourceRectUv, [...rgb(p.bg_colour), 1], [p.mirror ? 1 : 0, p.bg_mode === 'source' ? 1 : 0, 0, 0], this.src.view);
        this.fullscreen(enc, this.pBackground, bg, t.comp.view);

        // 分身
        if (mem && count > 0) {
            const cbg = this.device.createBindGroup({
                layout: this.compositeLayout,
                entries: [
                    { binding: 0, resource: { buffer: this.globals } },
                    { binding: 1, resource: mem.arrayView },
                    { binding: 2, resource: this.sampler },
                    { binding: 3, resource: this.lut.view },
                ],
            });
            const pass = enc.beginRenderPass({ colorAttachments: [{ view: t.comp.view, loadOp: 'load', storeOp: 'store' }] });
            pass.setBindGroup(0, cbg);
            pass.setVertexBuffer(0, this.instances);
            if (p.ground_shadows) {
                pass.setPipeline(this.pShadow);
                pass.draw(6, count);
            }
            pass.setPipeline(this.pComposite);
            pass.draw(6, count);
            pass.end();
        }

        // Bloom：亮部 → 1/2 模糊 → 1/4 模糊 → 疊回
        const strength = (p.bloom / 100) * 1.6;
        const zero = [0, 0, 0, 0];
        if (strength > 0) {
            const blur = (slot, dx, dy, from, to) => this.fullscreen(enc, this.pBlur, this.postBindGroup(slot, [dx, dy, 0, 0], zero, zero, from.view), to.view);
            this.fullscreen(enc, this.pBright, this.postBindGroup(SLOT.bright, [BLOOM_THRESHOLD, 0, 0, 0], zero, zero, t.comp.view), t.halfA.view);
            blur(SLOT.blurHalfH, 1 / t.halfA.width, 0, t.halfA, t.halfB);
            blur(SLOT.blurHalfV, 0, 1 / t.halfA.height, t.halfB, t.halfA);
            blur(SLOT.down, 0, 0, t.halfA, t.quarterA);
            blur(SLOT.blurQuarterH, 1 / t.quarterA.width, 0, t.quarterA, t.quarterB);
            blur(SLOT.blurQuarterV, 0, 1 / t.quarterA.height, t.quarterB, t.quarterA);
        }
        const [g1, g2] = strength > 0 ? [t.halfA.view, t.quarterA.view] : [t.comp.view, t.comp.view];
        const fin = this.postBindGroup(SLOT.final, [strength * 0.5, 0, 0, 0], zero, zero, t.comp.view, g1, g2);
        this.fullscreen(enc, this.pFinal, fin, this.context.getCurrentTexture().createView());

        queue.submit([enc.finish()]);
    }
}
