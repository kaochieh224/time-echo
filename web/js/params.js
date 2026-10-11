// 參數表：對應桌面版的 src/params.rs，JSON 欄位名稱相同，預設組可以互通。
// 數值一律以面板單位（%、秒、px、度）存放，換算成 0–1 的工作在 renderer／engine 內做。

// 手機版的解析度選項比桌面版低（記憶體與填充率）。
export const OUTPUT_RES = [720, 960, 1280];
export const BUFFER_RES = [320, 480, 640];
export const MATTE_SOURCES = ['ai', 'key', 'ai_key', 'luma', 'full'];
export const LAYOUTS = ['procession', 'centered', 'symmetric'];
export const SURFACES = ['textured', 'solid', 'contour'];
export const COLOUR_MODES = ['original', 'white', 'mono', 'custom'];
export const BG_MODES = ['solid', 'source'];
export const BEAT_DIVISIONS = [0.125, 0.25, 0.5, 1, 2];

/** 換預設組時保留的欄位：跟裝置與拍攝環境有關，不屬於「風格」。 */
const DEVICE_KEYS = ['mirror', 'matte_source', 'output_res', 'buffer_res', 'capture_rate', 'memory_length'];

export function defaults() {
    return {
        output_res: 960,
        mirror: true,
        matte_source: 'ai',
        buffer_res: 480,
        capture_rate: 15,
        memory_length: 8,
        echo_count: 12,
        interval: 0.3,
        spacing: 100,
        figure_size: 100,
        axis_x: 0,
        layout: 'procession',
        echo_fade: 0,
        surface: 'textured',
        disintegrate: 0,
        bloom: 0,
        colour_mode: 'original',
        custom_colour: '#FFFFFF',
        bg_mode: 'solid',
        bg_colour: '#000000',
        ground_shadows: false,
        shadow_opacity: 50,
        brightness: 0,
        contrast: 100,
        saturation: 100,
        curve_shadows: 0,
        curve_mids: 0,
        curve_highs: 0,
        duotone: false,
        duotone_dark: '#1A1AFF',
        duotone_light: '#FF000C',
        selective: false,
        selective_colour: '#FF000C',
        selective_tolerance: 30,
        selective_hue: 0,
        selective_sat: 0,
        selective_light: 0,
        screen_colour: '#00B140',
        screen_gain: 100,
        screen_balance: 50,
        despill: 0,
        clip_black: 0,
        clip_white: 100,
        shrink_grow: 0,
        edge_softness: 1,
        bpm: 120,
        beat_sync: false,
        beat_division: 0.5,
        beat_pulse: 0,
    };
}

/** 內建預設組（只列出和預設值不同的欄位）。 */
export const PRESETS = [
    { name: '預設', values: {} },
    {
        // 規格第 4 節「示範（TIME & MEMORY）」，和桌面版 Params::demo() 相同
        name: '示範',
        values: {
            echo_count: 22, interval: 0.3, spacing: 241, figure_size: 170, axis_x: 72,
            disintegrate: 100, bloom: 25, bg_colour: '#FF000C',
            brightness: -30, contrast: 160, saturation: 200, curve_shadows: 43, curve_highs: -25,
            screen_colour: '#000000', screen_gain: 107, screen_balance: 91, despill: 42,
            clip_black: 53, clip_white: 65, shrink_grow: -1, edge_softness: 2.5, bpm: 126,
        },
    },
    {
        name: '剪影',
        values: {
            echo_count: 16, interval: 0.2, spacing: 140, surface: 'solid', colour_mode: 'white',
            echo_fade: 85, bloom: 30, clip_black: 25, clip_white: 45,
        },
    },
    {
        name: '輪廓',
        values: {
            echo_count: 24, interval: 0.12, spacing: 120, surface: 'contour', colour_mode: 'custom',
            custom_colour: '#35E0FF', echo_fade: 70, bloom: 45, clip_black: 25, clip_white: 45,
        },
    },
];

const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
const nearest = (v, options) => options.reduce((a, b) => (Math.abs(b - v) < Math.abs(a - v) ? b : a));
const oneOf = (v, options, fallback) => (options.includes(v) ? v : fallback);

export function parseHex(s) {
    const m = /^#?([0-9a-f]{6})$/i.exec(String(s).trim());
    if (!m) return null;
    const v = parseInt(m[1], 16);
    return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}

/** `#RRGGBB` → 0–1 的 RGB；格式不對時回傳黑色。 */
export function rgb(s) {
    return (parseHex(s) ?? [0, 0, 0]).map((c) => c / 255);
}

/** 把超出範圍的值夾回規格範圍（就地修改）。 */
export function sanitize(p) {
    const d = defaults();
    p.output_res = nearest(p.output_res, OUTPUT_RES);
    p.buffer_res = nearest(p.buffer_res, BUFFER_RES);
    p.capture_rate = clamp(p.capture_rate, 10, 30);
    p.memory_length = clamp(p.memory_length, 2, 16);
    p.echo_count = clamp(Math.round(p.echo_count), 1, 48);
    p.interval = clamp(p.interval, 0.05, 2);
    p.spacing = clamp(p.spacing, -300, 300);
    p.figure_size = clamp(p.figure_size, 10, 300);
    p.axis_x = clamp(p.axis_x, -100, 100);
    p.echo_fade = clamp(p.echo_fade, 0, 100);
    p.disintegrate = clamp(p.disintegrate, 0, 100);
    p.bloom = clamp(p.bloom, 0, 100);
    p.brightness = clamp(p.brightness, -100, 100);
    p.contrast = clamp(p.contrast, 0, 200);
    p.saturation = clamp(p.saturation, 0, 200);
    p.curve_shadows = clamp(p.curve_shadows, -100, 100);
    p.curve_mids = clamp(p.curve_mids, -100, 100);
    p.curve_highs = clamp(p.curve_highs, -100, 100);
    p.clip_black = clamp(p.clip_black, 0, 99);
    p.clip_white = clamp(p.clip_white, p.clip_black + 1, 100);
    p.shrink_grow = Math.round(clamp(p.shrink_grow, -10, 10));
    p.edge_softness = clamp(p.edge_softness, 0, 10);
    p.bpm = clamp(p.bpm, 60, 200);
    p.beat_division = nearest(p.beat_division, BEAT_DIVISIONS);
    p.beat_pulse = clamp(p.beat_pulse, 0, 100);
    p.shadow_opacity = clamp(p.shadow_opacity, 0, 100);
    p.selective_tolerance = clamp(p.selective_tolerance, 5, 90);
    p.selective_hue = clamp(p.selective_hue, -180, 180);
    p.selective_sat = clamp(p.selective_sat, -100, 100);
    p.selective_light = clamp(p.selective_light, -100, 100);
    p.screen_gain = clamp(p.screen_gain, 0, 200);
    p.screen_balance = clamp(p.screen_balance, 0, 100);
    p.despill = clamp(p.despill, 0, 100);

    p.matte_source = oneOf(p.matte_source, MATTE_SOURCES, d.matte_source);
    p.layout = oneOf(p.layout, LAYOUTS, d.layout);
    p.surface = oneOf(p.surface, SURFACES, d.surface);
    p.colour_mode = oneOf(p.colour_mode, COLOUR_MODES, d.colour_mode);
    p.bg_mode = oneOf(p.bg_mode, BG_MODES, d.bg_mode);
    for (const k of Object.keys(d)) {
        if (typeof d[k] === 'string' && d[k].startsWith('#')) {
            const c = parseHex(p[k]);
            p[k] = c ? '#' + c.map((v) => v.toString(16).padStart(2, '0')).join('').toUpperCase() : d[k];
        }
    }
    return p;
}

/** 實際生效的時間間隔：開節拍同步時由 BPM 換算。 */
export function effectiveInterval(p) {
    return p.beat_sync ? (60 / p.bpm) * p.beat_division : p.interval;
}

/** 由任意物件建立參數：缺的欄位補預設值，型別不對或不認得的欄位丟掉。 */
export function fromObject(obj) {
    const p = defaults();
    for (const k of Object.keys(p)) {
        const v = obj?.[k];
        if (typeof v === typeof p[k] && (typeof v !== 'number' || Number.isFinite(v))) p[k] = v;
    }
    return sanitize(p);
}

export function fromJSON(text) {
    return fromObject(JSON.parse(text));
}

/** 套用預設組：風格欄位全部換掉，裝置相關的欄位與遮罩來源保留目前的值。 */
export function applyPreset(current, values) {
    const p = fromObject(values);
    for (const k of DEVICE_KEYS) p[k] = current[k];
    return sanitize(p);
}
