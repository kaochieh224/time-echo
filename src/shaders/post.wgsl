// 全畫面 pass：擷取、背景、Bloom、監看小窗共用同一組 binding。

struct P {
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
};

@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var tex0: texture_2d<f32>;
@group(0) @binding(2) var tex1: texture_2d<f32>;
@group(0) @binding(3) var tex2: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> VOut {
    let uv = vec2(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: VOut;
    o.pos = vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

fn mirror_uv(uv: vec2<f32>, m: f32) -> vec2<f32> {
    return select(uv, vec2(1.0 - uv.x, uv.y), m > 0.5);
}

// 擷取：來源 RGB ＋ 整理後的遮罩 → 記憶緩衝的一層。
// c.x = mirror；a.rgb = 幕色，b.x = Despill（0–1），b.y = 幕色是否中性
@fragment
fn fs_capture(in: VOut) -> @location(0) vec4<f32> {
    let uv = mirror_uv(in.uv, p.c.x);
    var rgb = textureSample(tex0, samp, uv).rgb;
    let m = textureSample(tex1, samp, uv).r;
    // 05 Despill：把幕色主通道壓到其餘兩通道的平均以下（中性幕不處理）
    if (p.b.x > 0.0 && p.b.y < 0.5) {
        let s = p.a.rgb;
        if (s.g >= s.r && s.g >= s.b) {
            rgb.g = mix(rgb.g, min(rgb.g, (rgb.r + rgb.b) * 0.5), p.b.x);
        } else if (s.b >= s.r) {
            rgb.b = mix(rgb.b, min(rgb.b, (rgb.r + rgb.g) * 0.5), p.b.x);
        } else {
            rgb.r = mix(rgb.r, min(rgb.r, (rgb.g + rgb.b) * 0.5), p.b.x);
        }
    }
    return vec4(rgb, m);
}

// 背景：a = 來源在畫面中的矩形（uv），b = 背景色，c.x = mirror，c.y = 模式（0 純色 / 1 原始畫面）
@fragment
fn fs_background(in: VOut) -> @location(0) vec4<f32> {
    let suv = (in.uv - p.a.xy) / max(p.a.zw - p.a.xy, vec2(1e-5));
    let src = textureSample(tex0, samp, mirror_uv(clamp(suv, vec2(0.0), vec2(1.0)), p.c.x)).rgb;
    let inside = all(suv >= vec2(0.0)) && all(suv <= vec2(1.0));
    if (p.c.y > 0.5 && inside) {
        return vec4(src, 1.0);
    }
    return vec4(p.b.rgb, 1.0);
}

// Bloom 亮部：a.x = 門檻
@fragment
fn fs_bright(in: VOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex0, samp, in.uv).rgb;
    let l = max(c.r, max(c.g, c.b));
    return vec4(c * smoothstep(p.a.x, p.a.x + 0.15, l), 1.0);
}

// 9-tap 高斯，a.xy = 一個 texel 的方向位移
@fragment
fn fs_blur(in: VOut) -> @location(0) vec4<f32> {
    let d = p.a.xy;
    var w = array<f32, 5>(0.2270, 0.1946, 0.1216, 0.0541, 0.0162);
    var acc = textureSample(tex0, samp, in.uv).rgb * w[0];
    for (var i = 1; i < 5; i++) {
        let o = d * f32(i) * 1.5;
        acc += textureSample(tex0, samp, in.uv + o).rgb * w[i];
        acc += textureSample(tex0, samp, in.uv - o).rgb * w[i];
    }
    return vec4(acc, 1.0);
}

// 最後合成：tex0 = 合成畫面，tex1/tex2 = 兩層模糊亮部，a.x = 強度
@fragment
fn fs_final(in: VOut) -> @location(0) vec4<f32> {
    let base = textureSample(tex0, samp, in.uv).rgb;
    let glow = textureSample(tex1, samp, in.uv).rgb + textureSample(tex2, samp, in.uv).rgb;
    return vec4(clamp(base + glow * p.a.x, vec3(0.0), vec3(1.0)), 1.0);
}

// 監看小窗：c.x = mirror，c.y = 0 原始 / 1 人物＋棋盤格 / 2 黑白遮罩，a.xy = 小窗尺寸 px
@fragment
fn fs_monitor(in: VOut) -> @location(0) vec4<f32> {
    let uv = mirror_uv(in.uv, p.c.x);
    let rgb = textureSample(tex0, samp, uv).rgb;
    let m = textureSample(tex1, samp, uv).r;
    if (p.c.y > 1.5) {
        return vec4(vec3(m), 1.0);
    }
    if (p.c.y > 0.5) {
        let cell = floor(in.uv * p.a.xy / 12.0);
        let checker = select(0.62, 0.82, (i32(cell.x + cell.y) & 1) == 0);
        return vec4(mix(vec3(checker), rgb, m), 1.0);
    }
    return vec4(rgb, 1.0);
}
