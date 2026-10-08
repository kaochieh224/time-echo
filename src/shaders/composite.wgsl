// 合成 pass：由舊到新畫 N 個分身 quad（規格 2 節流程）。
// 每個分身：Clip black/white → 表面模式 → Disintegrate → 調色（04）→ 色彩模式／雙色調。

struct Globals {
    out_size: vec2<f32>,
    clip: vec2<f32>,      // clip black, clip white（0–1）
    grade: vec4<f32>,     // brightness, contrast, saturation, noise scale
    modes: vec4<f32>,     // surface（0 紋理 / 1 剪影）, colour mode（0 原色 / 1 白 / 2 單色 / 3 自訂）, duotone, time
    custom: vec4<f32>,
    duo_dark: vec4<f32>,
    duo_light: vec4<f32>,
    extra: vec4<f32>,     // 記憶緩衝 texel 寬、高, 地面陰影不透明度, 未用
    sel: vec4<f32>,       // selective 開關, 目標色相（0–1）, 容差（0–0.5）, 色相位移（−0.5–0.5）
    sel2: vec4<f32>,      // 飽和度增減, 明度增減（−1–1）
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var mem: texture_2d_array<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var lut: texture_2d<f32>;

struct InstIn {
    @location(0) rect: vec4<f32>,
    @location(1) layer: u32,
    @location(2) erosion: f32,
    @location(3) opacity: f32,
    @location(4) seed: f32,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) @interpolate(flat) erosion: f32,
    @location(3) @interpolate(flat) opacity: f32,
    @location(4) @interpolate(flat) seed: f32,
    @location(5) @interpolate(flat) aspect: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: InstIn) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[vi];
    let px = mix(inst.rect.xy, inst.rect.zw, c);
    let ndc = vec2(px.x / g.out_size.x * 2.0 - 1.0, 1.0 - px.y / g.out_size.y * 2.0);
    var o: VOut;
    o.pos = vec4(ndc, 0.0, 1.0);
    o.uv = c;
    o.layer = inst.layer;
    o.erosion = inst.erosion;
    o.opacity = inst.opacity;
    o.seed = inst.seed;
    let size = inst.rect.zw - inst.rect.xy;
    o.aspect = size.x / max(size.y, 1.0);
    return o;
}

fn hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2(1.0, 0.0));
    let c = hash(i + vec2(0.0, 1.0));
    let d = hash(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    return value_noise(p) * 0.6 + value_noise(p * 2.7 + 11.0) * 0.3 + value_noise(p * 7.1 + 37.0) * 0.1;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3(0.2126, 0.7152, 0.0722));
}

fn rgb2hsv(c: vec3<f32>) -> vec3<f32> {
    let k = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = mix(vec4(c.bg, k.wz), vec4(c.gb, k.xy), step(c.b, c.g));
    let q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
    let d = q.x - min(q.w, q.y);
    let e = 1.0e-10;
    return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

fn hsv2rgb(c: vec3<f32>) -> vec3<f32> {
    let k = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + k.xyz) * 6.0 - k.www);
    return c.z * mix(k.xxx, clamp(p - k.xxx, vec3(0.0), vec3(1.0)), c.y);
}

// 04 Selective colour：只動接近目標色相的畫素
fn selective(rgb: vec3<f32>) -> vec3<f32> {
    if (g.sel.x < 0.5) {
        return rgb;
    }
    var hsv = rgb2hsv(rgb);
    let dh = abs(hsv.x - g.sel.y);
    let d = min(dh, 1.0 - dh);
    let w = (1.0 - smoothstep(g.sel.z * 0.6, g.sel.z, d)) * smoothstep(0.08, 0.2, hsv.y);
    hsv.x = fract(hsv.x + g.sel.w * w + 1.0);
    hsv.y = clamp(hsv.y * (1.0 + g.sel2.x * w), 0.0, 1.0);
    var out = hsv2rgb(hsv);
    let l = g.sel2.y * w;
    out = select(mix(out, vec3(0.0), -l), mix(out, vec3(1.0), l), l > 0.0);
    return out;
}

fn clip_alpha(a: f32) -> f32 {
    return clamp((a - g.clip.x) / max(g.clip.y - g.clip.x, 0.001), 0.0, 1.0);
}

fn curve(c: vec3<f32>) -> vec3<f32> {
    let i = vec3<i32>(clamp(c, vec3(0.0), vec3(1.0)) * 255.0 + 0.5);
    return vec3(
        textureLoad(lut, vec2(i.r, 0), 0).r,
        textureLoad(lut, vec2(i.g, 0), 0).r,
        textureLoad(lut, vec2(i.b, 0), 0).r,
    );
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let s = textureSample(mem, samp, in.uv, in.layer);
    // 輪廓模式要用的鄰近取樣（先取，避免非一致控制流裡取樣）
    let t = g.extra.xy * 1.5;
    let al = clip_alpha(textureSample(mem, samp, in.uv - vec2(t.x, 0.0), in.layer).a);
    let ar = clip_alpha(textureSample(mem, samp, in.uv + vec2(t.x, 0.0), in.layer).a);
    let au = clip_alpha(textureSample(mem, samp, in.uv - vec2(0.0, t.y), in.layer).a);
    let ad = clip_alpha(textureSample(mem, samp, in.uv + vec2(0.0, t.y), in.layer).a);

    // 05 Clip black / white
    var a = clip_alpha(s.a);
    let surface = g.modes.x;
    if (surface > 1.5) {
        a = clamp((abs(ar - al) + abs(ad - au)) * 1.6, 0.0, 1.0);
    }

    // 03 Disintegrate：越舊侵蝕越多，雜訊黏在分身上
    if (in.erosion > 0.001) {
        let n = fbm(in.uv * vec2(in.aspect, 1.0) * g.grade.w + vec2(in.seed * 17.13, in.seed * 5.71));
        a = a * smoothstep(in.erosion - 0.06, in.erosion + 0.06, n * 1.08 - 0.04);
    }

    let colour_mode = g.modes.y;
    var rgb: vec3<f32>;
    if (surface > 0.5) {
        // 實心剪影／輪廓：填色彩模式的顏色，原色模式時填白色
        rgb = select(vec3(1.0), g.custom.rgb, colour_mode > 2.5);
        if (g.modes.z > 0.5) {
            rgb = g.duo_light.rgb;
        }
    } else {
        // 04 調色：Brightness → Contrast → Saturation → 曲線
        rgb = s.rgb + 0.5 * g.grade.x;
        rgb = (rgb - 0.5) * g.grade.y + 0.5;
        rgb = mix(vec3(luma(rgb)), rgb, g.grade.z);
        rgb = curve(clamp(rgb, vec3(0.0), vec3(1.0)));
        rgb = selective(rgb);
        let l = luma(rgb);
        if (g.modes.z > 0.5) {
            rgb = mix(g.duo_dark.rgb, g.duo_light.rgb, l);
        } else if (colour_mode > 2.5) {
            rgb = g.custom.rgb * mix(0.35, 1.0, l);
        } else if (colour_mode > 1.5) {
            rgb = vec3(l);
        } else if (colour_mode > 0.5) {
            rgb = mix(vec3(l), vec3(1.0), 0.6);
        }
    }
    return vec4(rgb, a * in.opacity);
}

// 03 Ground shadows：剪影壓扁、往後上方斜躺在腳下，模糊成影子
@vertex
fn vs_shadow(@builtin(vertex_index) vi: u32, inst: InstIn) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[vi];
    let size = inst.rect.zw - inst.rect.xy;
    let feet = inst.rect.w;
    var px = vec2(mix(inst.rect.x, inst.rect.z, c.x), feet - size.y * 0.16 * (1.0 - c.y));
    px.x = px.x + (1.0 - c.y) * size.x * 0.22;
    let ndc = vec2(px.x / g.out_size.x * 2.0 - 1.0, 1.0 - px.y / g.out_size.y * 2.0);
    var o: VOut;
    o.pos = vec4(ndc, 0.0, 1.0);
    o.uv = c;
    o.layer = inst.layer;
    o.erosion = inst.erosion;
    o.opacity = inst.opacity;
    o.seed = inst.seed;
    o.aspect = size.x / max(size.y, 1.0);
    return o;
}

@fragment
fn fs_shadow(in: VOut) -> @location(0) vec4<f32> {
    var acc = 0.0;
    for (var i = -2; i <= 2; i++) {
        for (var j = -2; j <= 2; j++) {
            let o = vec2(f32(i) * 0.012, f32(j) * 0.03);
            acc += clip_alpha(textureSample(mem, samp, clamp(in.uv + o, vec2(0.0), vec2(1.0)), in.layer).a);
        }
    }
    let a = acc / 25.0 * g.extra.z * 0.85 * in.opacity;
    return vec4(0.0, 0.0, 0.0, a);
}
