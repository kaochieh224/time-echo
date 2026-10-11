// 手機版前端：來源、主迴圈與控制面板。對應桌面版 src/app.rs 的角色。

import { analyseFile } from './audio.js';
import { Engine } from './engine.js';
import { buildMask } from './matte.js';
import { BUFFER_RES, OUTPUT_RES, PRESETS, applyPreset, defaults, effectiveInterval, fromJSON, fromObject, sanitize } from './params.js';
import { Recorder, canRecord } from './recorder.js';
import { Renderer } from './renderer.js';
import { PersonSegmenter } from './segment.js';
import { DemoSource, MAX_SOURCE_EDGE, openCamera, openFile } from './sources.js';

const STORAGE_KEY = 'time-echo-web-params';
const $ = (id) => document.getElementById(id);

let p = loadParams();
let engine = null;
let source = null;
let facing = 'user';
let frozen = false;
let lastPush = null;
let stopped = false;
let recorder = null;
/** 上一段錄好的影片（等使用者分享或下載） */
let recording = null;
/** 匯入的音樂：name 是檔名；ctx／tap 是把音樂接進錄影用的 WebAudio 節點 */
const music = { name: '', url: '', ctx: null, tap: null };
/** AI 分割器的狀態：idle → loading → ready／failed */
let ai = { state: 'idle', segmenter: null, error: '' };

// 來源太大時先縮到這張 canvas；遮罩用的畫素從 maskCanvas 讀
const scaleCanvas = document.createElement('canvas');
const maskCanvas = document.createElement('canvas');
const maskCtx = maskCanvas.getContext('2d', { willReadFrequently: true });

// ───────────── 小工具 ─────────────

function el(tag, props = {}, ...children) {
    const node = Object.assign(document.createElement(tag), props);
    node.append(...children);
    return node;
}

function loadParams() {
    try {
        const saved = localStorage.getItem(STORAGE_KEY);
        if (saved) return fromJSON(saved);
    } catch {
        // 存的內容壞了或不能用 localStorage：用預設值
    }
    return defaults();
}

let saveTimer = 0;
function saveParams() {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
        try {
            localStorage.setItem(STORAGE_KEY, JSON.stringify(p));
        } catch {
            // 無痕模式等情況存不了，不影響使用
        }
    }, 300);
}

let toastTimer = 0;
function toast(message) {
    const t = $('toast');
    t.textContent = message;
    t.classList.add('show');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => t.classList.remove('show'), 3500);
}

function fatal(message) {
    stopped = true;
    const msg = $('start-msg');
    msg.textContent = message;
    msg.classList.add('error');
    $('start').hidden = false;
    for (const b of document.querySelectorAll('.start-buttons button, #bar button')) b.disabled = true;
}

// ───────────── 控制項 ─────────────

/** 每個控制項註冊一個「依目前參數更新畫面」的函式 */
const refreshers = [];

function changed() {
    sanitize(p);
    ensureAi();
    refresh();
    saveParams();
}

const usesAi = (q) => q.matte_source === 'ai' || q.matte_source === 'ai_key';

/** 遮罩來源用到 AI 時才下載函式庫與模型（只試一次，失敗後要重新整理）。 */
function ensureAi() {
    if (!usesAi(p) || ai.state !== 'idle') return;
    ai.state = 'loading';
    PersonSegmenter.create().then(
        (segmenter) => {
            ai = { state: 'ready', segmenter, error: '' };
            toast('AI 人物去背已就緒');
            refresh();
        },
        (e) => {
            console.error(e);
            ai = { state: 'failed', segmenter: null, error: e.message };
            toast('AI 模型載入失敗，先用全畫面；可以改用亮度或色鍵');
            refresh();
        },
    );
}

function aiStatusText() {
    if (!usesAi(p)) return '';
    return {
        idle: '',
        loading: 'AI 模型下載中…（下載完成前先顯示全畫面）',
        ready: `AI 人物去背已就緒（${ai.segmenter?.delegate}）`,
        failed: `AI 模型載入失敗：${ai.error}`,
    }[ai.state];
}

function refresh() {
    for (const f of refreshers) f();
}

/** `opts.when(p)`：回傳 false 時隱藏；`opts.text(p)`：自訂數值文字；`opts.disabled(p)` */
function slider(key, label, min, max, step, unit = '', opts = {}) {
    const decimals = (String(step).split('.')[1] ?? '').length;
    const out = el('output');
    const input = el('input', { type: 'range', min, max, step });
    input.setAttribute('aria-label', label);
    input.addEventListener('input', () => {
        p[key] = Number(input.value);
        changed();
    });
    const node = el('div', { className: 'ctl' }, el('div', { className: 'ctl-head' }, el('span', {}, label), out), input);
    refreshers.push(() => {
        node.hidden = opts.when ? !opts.when(p) : false;
        input.disabled = opts.disabled ? opts.disabled(p) : false;
        input.value = p[key];
        out.textContent = opts.text ? opts.text(p) : p[key].toFixed(decimals) + unit;
    });
    return node;
}

/** 單選一排按鈕。`options` = [[值, 標籤], …] */
function choice(key, label, options, opts = {}) {
    const buttons = options.map(([value, text]) => {
        const b = el('button', { type: 'button' }, text);
        b.addEventListener('click', () => {
            p[key] = value;
            changed();
        });
        return [value, b];
    });
    const node = el('div', { className: 'ctl' }, el('div', { className: 'ctl-head' }, el('span', {}, label)), el('div', { className: 'row' }, ...buttons.map(([, b]) => b)));
    refreshers.push(() => {
        node.hidden = opts.when ? !opts.when(p) : false;
        for (const [value, b] of buttons) b.setAttribute('aria-pressed', String(p[key] === value));
    });
    return node;
}

function check(key, label, opts = {}) {
    const input = el('input', { type: 'checkbox' });
    input.addEventListener('change', () => {
        p[key] = input.checked;
        changed();
    });
    const node = el('label', { className: 'check ctl' }, input, label);
    refreshers.push(() => {
        node.hidden = opts.when ? !opts.when(p) : false;
        input.checked = p[key];
    });
    return node;
}

function colour(key, label, opts = {}) {
    const input = el('input', { type: 'color' });
    input.addEventListener('input', () => {
        p[key] = input.value;
        changed();
    });
    const node = el('label', { className: 'colour ctl' }, label, input);
    refreshers.push(() => {
        node.hidden = opts.when ? !opts.when(p) : false;
        input.value = p[key].toLowerCase();
    });
    return node;
}

const group = (text) => el('div', { className: 'group' }, text);
const hint = (text) => el('p', { className: 'hint' }, text);

/** 內容會跟著狀態變的說明文字；空字串時隱藏。 */
function liveHint(text) {
    const node = el('p', { className: 'hint ctl' });
    refreshers.push(() => {
        node.textContent = text();
        node.hidden = node.textContent === '';
    });
    return node;
}

/** 一排動作按鈕。`items` = [[標籤, onClick], …]；標籤可以是函式（跟著狀態變）。 */
function actions(...items) {
    return el(
        'div',
        { className: 'row ctl' },
        ...items.map(([text, onClick]) => {
            const b = el('button', { type: 'button' });
            if (typeof text === 'function') refreshers.push(() => (b.textContent = text()));
            else b.textContent = text;
            b.addEventListener('click', onClick);
            return b;
        }),
    );
}

function buildPanel() {
    const usesKey = (q) => q.matte_source === 'key' || q.matte_source === 'ai_key';
    const hasMatte = (q) => q.matte_source !== 'full';
    const pages = [
        [
            '常用',
            group('預設組'),
            actions(
                ...PRESETS.map(({ name, values }) => [
                    name,
                    () => {
                        p = applyPreset(p, values);
                        changed();
                        toast(`已套用預設組：${name}`);
                    },
                ]),
            ),
            group('時間與空間'),
            slider('echo_count', '分身數量', 1, 48, 1),
            slider('interval', '時間間隔', 0.05, 2, 0.01, ' s', {
                disabled: (q) => q.beat_sync,
                text: (q) => (q.beat_sync ? `${effectiveInterval(q).toFixed(3)} s ♪` : `${q.interval.toFixed(2)} s`),
            }),
            slider('spacing', '分身間距', -300, 300, 1, ' %'),
            slider('figure_size', '人物大小', 10, 300, 1, ' %'),
            slider('axis_x', '水平位置', -100, 100, 1),
            slider('disintegrate', '崩解', 0, 100, 1, ' %'),
            slider('bloom', '光暈', 0, 100, 1, ' %'),
        ],
        [
            '外觀',
            choice('layout', '排列', [['procession', '層疊行進'], ['centered', '置中'], ['symmetric', '鏡像對稱']]),
            choice('surface', '表面', [['textured', '影像分身'], ['solid', '實心剪影'], ['contour', '輪廓']]),
            slider('echo_fade', '分身淡出', 0, 100, 1, ' %'),
            choice('colour_mode', '色彩', [['original', '原始色彩'], ['white', '白色'], ['mono', '單色'], ['custom', '自訂顏色']]),
            colour('custom_colour', '自訂顏色', { when: (q) => q.colour_mode === 'custom' }),
            choice('bg_mode', '背景', [['solid', '純色'], ['source', '原始畫面']]),
            colour('bg_colour', '背景顏色', { when: (q) => q.bg_mode === 'solid' }),
            check('ground_shadows', '地面陰影'),
            slider('shadow_opacity', '陰影濃度', 0, 100, 1, ' %', { when: (q) => q.ground_shadows }),
        ],
        [
            '調色',
            slider('brightness', '亮度', -100, 100, 1, ' %'),
            slider('contrast', '對比', 0, 200, 1, ' %'),
            slider('saturation', '飽和度', 0, 200, 1, ' %'),
            group('RGB 曲線'),
            slider('curve_shadows', '暗部', -100, 100, 1, ' %'),
            slider('curve_mids', '中間調', -100, 100, 1, ' %'),
            slider('curve_highs', '亮部', -100, 100, 1, ' %'),
            group('雙色調'),
            check('duotone', '雙色調'),
            colour('duotone_dark', '暗部顏色', { when: (q) => q.duotone }),
            colour('duotone_light', '亮部顏色', { when: (q) => q.duotone }),
            group('選取顏色'),
            check('selective', '只調整選取的顏色'),
            colour('selective_colour', '選取的顏色', { when: (q) => q.selective }),
            slider('selective_tolerance', '色相範圍', 5, 90, 1, '°', { when: (q) => q.selective }),
            slider('selective_hue', '色相位移', -180, 180, 1, '°', { when: (q) => q.selective }),
            slider('selective_sat', '選色飽和度', -100, 100, 1, ' %', { when: (q) => q.selective }),
            slider('selective_light', '明度', -100, 100, 1, ' %', { when: (q) => q.selective }),
            actions([
                '重設調色',
                () => {
                    const d = defaults();
                    for (const k of Object.keys(d)) {
                        if (/^(brightness|contrast|saturation|curve_|duotone|selective)/.test(k)) p[k] = d[k];
                    }
                    changed();
                },
            ]),
        ],
        [
            '去背',
            choice('matte_source', '遮罩來源', [['ai', 'AI 人物'], ['key', '色鍵'], ['ai_key', 'AI × 色鍵'], ['luma', '亮度'], ['full', '全畫面']]),
            liveHint(aiStatusText),
            hint('AI 人物適合一般場景；亮度遮罩適合深色背景。'),
            slider('clip_black', '黑階裁切', 0, 99, 1, ' %', { when: hasMatte }),
            slider('clip_white', '白階裁切', 1, 100, 1, ' %', { when: hasMatte }),
            slider('shrink_grow', '收縮／擴張', -10, 10, 1, ' px', { when: hasMatte }),
            slider('edge_softness', '邊緣柔化', 0, 10, 0.5, ' px', { when: hasMatte }),
            colour('screen_colour', '幕色', { when: usesKey }),
            slider('screen_gain', '幕色增益', 0, 200, 1, ' %', { when: usesKey }),
            slider('screen_balance', '幕色平衡', 0, 100, 1, ' %', { when: usesKey }),
            slider('despill', '去溢色', 0, 100, 1, ' %', { when: usesKey }),
        ],
        [
            '設定',
            check('mirror', '鏡像畫面'),
            choice('output_res', '輸出解析度', OUTPUT_RES.map((v) => [v, String(v)])),
            group('記憶緩衝（按「重設記憶」生效）'),
            choice('buffer_res', '緩衝長邊', BUFFER_RES.map((v) => [v, String(v)])),
            slider('capture_rate', '取樣率', 10, 30, 1, ' fps'),
            slider('memory_length', '記憶長度', 2, 16, 0.5, ' s'),
            group('節拍'),
            check('beat_sync', '節拍同步（時間間隔跟著 BPM）'),
            slider('bpm', 'BPM', 60, 200, 1),
            choice('beat_division', '拍數', [[0.125, '1/8'], [0.25, '1/4'], [0.5, '1/2'], [1, '1'], [2, '2']], { when: (q) => q.beat_sync }),
            slider('beat_pulse', '節拍脈衝', 0, 100, 1, ' %'),
            actions(
                [
                    '打拍',
                    () => {
                        const bpm = engine?.beat.tap(beatTime(performance.now() / 1000));
                        if (bpm) {
                            p.bpm = Math.round(bpm);
                            changed();
                        }
                    },
                ],
                ['×2', () => scaleBpm(2)],
                ['÷2', () => scaleBpm(0.5)],
            ),
            group('音樂'),
            actions(['匯入音樂', () => $('file-music').click()], [() => ($('music').paused ? '播放音樂' : '暫停音樂'), toggleMusic]),
            liveHint(() => (music.name ? `${music.name}（錄影時會一起錄進去）` : '匯入音樂會自動估 BPM；不準可以按 ×2／÷2 或「打拍」。')),
            group('預設組檔案（和桌面版的 JSON 通用）'),
            actions(['匯入 JSON', () => $('file-preset').click()], ['匯出 JSON', exportPreset]),
        ],
    ];

    const tabs = $('tabs');
    const container = $('pages');
    const entries = pages.map(([label, ...controls], i) => {
        const page = el('div', { className: 'page', id: `page-${i}` }, ...controls);
        page.setAttribute('role', 'tabpanel');
        const tab = el('button', { type: 'button' }, label);
        tab.setAttribute('role', 'tab');
        tab.setAttribute('aria-controls', page.id);
        tab.addEventListener('click', () => select(i));
        tabs.append(tab);
        container.append(page);
        return [tab, page];
    });
    const select = (index) => {
        entries.forEach(([tab, page], i) => {
            tab.setAttribute('aria-selected', String(i === index));
            page.hidden = i !== index;
        });
        container.scrollTop = 0;
    };
    select(0);
}

function exportPreset() {
    const url = URL.createObjectURL(new Blob([JSON.stringify(p, null, 2)], { type: 'application/json' }));
    el('a', { href: url, download: 'time-echo-preset.json' }).click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// ───────────── 音樂與節拍 ─────────────

/** 節拍時鐘用的時間：音樂在播就跟著音樂的位置，否則用牆上時間。 */
function beatTime(now) {
    const audio = $('music');
    return music.name && !audio.paused ? audio.currentTime : now;
}

function scaleBpm(factor) {
    const bpm = p.bpm * factor;
    if (bpm < 60 || bpm > 200) return;
    p.bpm = bpm;
    changed();
}

async function loadMusic(file) {
    const audio = $('music');
    audio.pause();
    if (music.url) URL.revokeObjectURL(music.url);
    music.url = URL.createObjectURL(file);
    music.name = file.name;
    audio.src = music.url;
    // 拍點對齊音樂開頭
    engine.beat.origin = 0;
    refresh();
    toast(`分析 ${file.name} 的 BPM…`);
    const bpm = await analyseFile(file);
    if (music.name !== file.name) return;
    if (bpm) {
        p.bpm = bpm;
        changed();
        toast(`已匯入 ${file.name}，估計 ${bpm.toFixed(1)} BPM`);
    } else {
        toast(`已匯入 ${file.name}，估不出 BPM，請用「打拍」`);
    }
}

/** 第一次播放時把音樂接到 WebAudio，錄影才拿得到音軌；不支援就只播放、不錄音。 */
function connectMusic() {
    if (music.ctx) return;
    const Ctx = window.AudioContext ?? window.webkitAudioContext;
    if (!Ctx) return;
    try {
        music.ctx = new Ctx();
        const node = music.ctx.createMediaElementSource($('music'));
        music.tap = music.ctx.createMediaStreamDestination();
        node.connect(music.ctx.destination);
        node.connect(music.tap);
    } catch (e) {
        console.error(e);
    }
}

async function toggleMusic() {
    const audio = $('music');
    if (!music.name) {
        toast('還沒匯入音樂');
        return;
    }
    if (audio.paused) {
        connectMusic();
        await music.ctx?.resume();
        await audio.play().catch(() => toast('這個音樂格式瀏覽器不能播放'));
    } else {
        audio.pause();
    }
    refresh();
}

// ───────────── 錄影 ─────────────

function updateRecordUi() {
    const rec = $('btn-rec');
    rec.disabled = source === null;
    rec.hidden = recording !== null;
    rec.textContent = recorder ? '停止' : '錄影';
    rec.setAttribute('aria-pressed', String(recorder !== null));
    $('rec-result').hidden = recording === null;
}

async function toggleRecording() {
    if (!recorder) {
        const canvas = $('out');
        if (!canRecord(canvas)) {
            toast('這個瀏覽器不支援錄影');
            return;
        }
        try {
            const audioTrack = $('music').paused ? null : (music.tap?.stream.getAudioTracks()[0] ?? null);
            recorder = new Recorder(canvas, audioTrack);
        } catch (e) {
            toast(`無法開始錄影：${e.message}`);
        }
        updateRecordUi();
        return;
    }
    const active = recorder;
    recorder = null;
    try {
        const { file, seconds } = await active.stop();
        recording = { file, url: URL.createObjectURL(file) };
        $('rec-info').textContent = `錄好了：${seconds.toFixed(0)} 秒，${(file.size / 1e6).toFixed(1)} MB（${file.name.split('.').pop().toUpperCase()}）`;
        $('link-download').href = recording.url;
        $('link-download').download = file.name;
        $('btn-share').hidden = !navigator.canShare?.({ files: [file] });
    } catch (e) {
        toast(`錄影失敗：${e.message}`);
    }
    updateRecordUi();
}

// ───────────── 來源 ─────────────

function setSource(next) {
    source?.stop();
    source = next;
    frozen = false;
    lastPush = null;
    $('start').hidden = true;
    updateBar();
    // 螢幕不要自動關掉（不支援就算了）
    navigator.wakeLock?.request('screen').catch(() => {});
}

async function startCamera() {
    try {
        const cam = await openCamera($('feed'), facing);
        // 前鏡頭照鏡子比較直覺，後鏡頭不鏡像
        p.mirror = facing === 'user';
        changed();
        setSource(cam);
    } catch (e) {
        toast(e.name === 'NotAllowedError' ? '沒有攝影機權限，請在瀏覽器設定裡允許' : `無法開啟攝影機：${e.message}`);
    }
}

function updateBar() {
    const has = source !== null;
    $('btn-freeze').disabled = !has;
    $('btn-freeze').textContent = frozen ? '繼續' : '凍結';
    $('btn-freeze').setAttribute('aria-pressed', String(frozen));
    $('btn-flip').disabled = source?.kind !== 'camera';
    $('btn-reset').disabled = !has;
    $('btn-source').disabled = !has;
    updateRecordUi();
}

function bindEvents() {
    $('btn-camera').addEventListener('click', startCamera);
    $('btn-demo').addEventListener('click', () => {
        // 示範動畫是黑底上的火柴人，AI 認不出人，改用亮度遮罩
        if (usesAi(p)) {
            p.matte_source = 'luma';
            changed();
            toast('示範動畫不是真人，遮罩來源改成「亮度」');
        }
        setSource(new DemoSource());
    });
    $('btn-file').addEventListener('click', () => $('file-video').click());
    $('file-video').addEventListener('change', async (e) => {
        const file = e.target.files[0];
        e.target.value = '';
        if (!file) return;
        // 新影片會蓋掉同一個 video 元素，先停掉舊來源
        source?.stop();
        source = null;
        try {
            setSource(await openFile($('feed'), file));
        } catch (err) {
            toast(err.message);
            $('start').hidden = false;
            updateBar();
        }
    });
    $('btn-freeze').addEventListener('click', () => {
        frozen = !frozen;
        lastPush = null;
        source?.setPaused(frozen);
        updateBar();
    });
    $('btn-flip').addEventListener('click', () => {
        facing = facing === 'user' ? 'environment' : 'user';
        source?.stop();
        source = null;
        startCamera();
    });
    $('btn-reset').addEventListener('click', () => {
        engine.resetMemory(p);
        toast('記憶已重設');
    });
    $('btn-source').addEventListener('click', () => {
        $('start').hidden = false;
    });
    $('btn-panel').addEventListener('click', () => {
        const panel = $('panel');
        panel.hidden = !panel.hidden;
        $('btn-panel').textContent = panel.hidden ? '展開面板' : '收起面板';
        $('btn-panel').setAttribute('aria-expanded', String(!panel.hidden));
    });
    $('file-music').addEventListener('change', (e) => {
        const file = e.target.files[0];
        e.target.value = '';
        if (file) loadMusic(file);
    });
    for (const type of ['play', 'pause']) $('music').addEventListener(type, refresh);
    $('btn-rec').addEventListener('click', toggleRecording);
    $('btn-share').addEventListener('click', async () => {
        try {
            await navigator.share({ files: [recording.file], title: 'TIME ECHO' });
        } catch (e) {
            // 使用者自己取消分享不算錯誤
            if (e.name !== 'AbortError') toast('分享失敗，請改用「下載」');
        }
    });
    $('btn-rec-close').addEventListener('click', () => {
        URL.revokeObjectURL(recording.url);
        recording = null;
        updateRecordUi();
    });
    $('file-preset').addEventListener('change', async (e) => {
        const file = e.target.files[0];
        e.target.value = '';
        if (!file) return;
        try {
            p = applyPreset(p, JSON.parse(await file.text()));
            changed();
            toast('已匯入預設組（解析度、緩衝與鏡像維持目前的設定）');
        } catch {
            toast('讀不了這個預設組');
        }
    });
}

// ───────────── 主迴圈 ─────────────

/** 來源長邊超過上限時先縮小，回傳 [drawable, 寬, 高]。 */
function sourceDrawable() {
    const { width: w, height: h, drawable } = source;
    const long = Math.max(w, h);
    if (long <= MAX_SOURCE_EDGE) return [drawable, w, h];
    const s = MAX_SOURCE_EDGE / long;
    const sw = Math.round((w * s) / 2) * 2;
    const sh = Math.round((h * s) / 2) * 2;
    if (scaleCanvas.width !== sw || scaleCanvas.height !== sh) {
        scaleCanvas.width = sw;
        scaleCanvas.height = sh;
    }
    scaleCanvas.getContext('2d').drawImage(drawable, 0, 0, sw, sh);
    return [scaleCanvas, sw, sh];
}

function makeMask(drawable, bw, bh) {
    // 來源先縮到緩衝解析度：色鍵與亮度讀它的畫素，AI 也吃這張小圖（模型輸出和輸入同尺寸）
    let drawn = false;
    const shrink = () => {
        if (drawn) return;
        drawn = true;
        if (maskCanvas.width !== bw || maskCanvas.height !== bh) {
            maskCanvas.width = bw;
            maskCanvas.height = bh;
        }
        maskCtx.drawImage(drawable, 0, 0, bw, bh);
    };
    const readPixels = () => {
        shrink();
        return maskCtx.getImageData(0, 0, bw, bh).data;
    };
    const segmenter = ai.segmenter;
    const segment = () => {
        shrink();
        return segmenter.segment(maskCanvas, performance.now());
    };
    return buildMask(p, bw, bh, readPixels, segmenter && segment);
}

let fpsFrames = 0;
let fpsSince = 0;
let fps = 0;

function updateHud(now) {
    fpsFrames++;
    if (now - fpsSince < 0.5) return;
    fps = fpsFrames / (now - fpsSince);
    fpsFrames = 0;
    fpsSince = now;

    const { ring, lastPlan: plan } = engine;
    const rec = recorder ? ` · ● 錄影 ${recorder.seconds.toFixed(0)} 秒` : '';
    $('status').textContent = `記憶 ${ring.count}/${ring.capacity} 張 · ${fps.toFixed(0)} FPS${frozen ? ' · 已凍結' : ''}${rec}`;
    const notes = [];
    if (plan.beyondCapacity > 0) notes.push(`有 ${plan.beyondCapacity} 個分身超出記憶長度`);
    if (plan.intervalTooSmall) notes.push('間隔小於取樣間隔，相鄰分身會取到同一張');
    const pending = engine.bufferSettingsPending(p);
    if (pending) notes.push('緩衝設定已改，按「重設記憶」生效');
    $('btn-reset').classList.toggle('pending', pending);
    $('notes').replaceChildren(...notes.map((n) => el('li', {}, n)));
}

function frame(ms) {
    if (stopped) return;
    const now = ms / 1000;
    try {
        if (source && source.ready() && source.takeFrame(now) && !frozen) {
            const dt = lastPush === null ? 0 : Math.min(0.25, now - lastPush);
            lastPush = now;
            const [drawable, w, h] = sourceDrawable();
            engine.pushFrame(p, drawable, w, h, dt, (bw, bh) => makeMask(drawable, bw, bh));
        }
        if (engine.srcSize) {
            engine.render(p, beatTime(now));
            recorder?.frame();
            updateHud(now);
        }
    } catch (e) {
        console.error(e);
        fatal(`算圖發生錯誤：${e.message}`);
        return;
    }
    requestAnimationFrame(frame);
}

async function init() {
    buildPanel();
    bindEvents();
    refresh();
    ensureAi();

    if (!navigator.gpu) {
        fatal('這個瀏覽器不支援 WebGPU。請用 Android 的 Chrome 121 以上或 iOS 26 以上的 Safari，並確認網址是 https。');
        return;
    }
    const adapter = await navigator.gpu.requestAdapter();
    const device = await adapter?.requestDevice();
    if (!device) {
        fatal('這台裝置拿不到可用的 GPU（WebGPU adapter）。');
        return;
    }
    device.lost.then((info) => fatal(`GPU 裝置中斷了，請重新整理頁面。（${info.message || info.reason}）`));
    device.addEventListener('uncapturederror', (e) => {
        console.error(e.error);
        toast(`GPU 錯誤：${e.error.message}`);
    });

    const canvas = $('out');
    const context = canvas.getContext('webgpu');
    const format = navigator.gpu.getPreferredCanvasFormat();
    context.configure({ device, format, alphaMode: 'opaque' });

    const load = async (name) => {
        const res = await fetch(new URL(`../shaders/${name}`, import.meta.url));
        if (!res.ok) throw new Error(`讀不到 ${name}`);
        return res.text();
    };
    const renderer = new Renderer(device, context, format, { post: await load('post.wgsl'), composite: await load('composite.wgsl') });
    const errors = await renderer.shaderErrors();
    if (errors.length) {
        fatal(`Shader 編譯失敗：${errors[0]}`);
        return;
    }
    engine = new Engine(renderer, p);
    requestAnimationFrame(frame);
}

init().catch((e) => {
    console.error(e);
    fatal(`啟動失敗：${e.message}`);
});

// 給除錯用：在主控台可以看目前的參數與引擎狀態
window.timeEcho = { get params() { return p; }, get engine() { return engine; }, get ai() { return ai; }, fromObject };
