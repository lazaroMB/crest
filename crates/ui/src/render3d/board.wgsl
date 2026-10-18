// The board renderer: a lit scene pass into HDR colour and normal/depth
// targets, then full-screen passes for the edge outlines, the bloom
// (bright pass, two blurs) and the tone-mapped composite.

struct Camera {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    light: vec4<f32>,
    // x: 1 / far distance, y: ambient boost.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) emissive: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) emissive: vec3<f32>,
};

@vertex
fn vs_scene(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = cam.view_proj * vec4<f32>(v.pos, 1.0);
    o.world = v.pos;
    o.normal = v.normal;
    o.color = v.color;
    o.emissive = v.emissive;
    return o;
}

struct SceneOut {
    @location(0) color: vec4<f32>,
    @location(1) nd: vec4<f32>,
};

@fragment
fn fs_scene(i: VsOut) -> SceneOut {
    let n = normalize(i.normal);
    let l = normalize(cam.light.xyz);
    let v = normalize(cam.eye.xyz - i.world);
    let h = normalize(l + v);
    let diffuse = max(dot(n, l), 0.0);
    let specular = pow(max(dot(n, h), 0.0), 40.0) * 0.30;
    let sky = vec3<f32>(0.20, 0.25, 0.32);
    let ground = vec3<f32>(0.04, 0.05, 0.06);
    let ambient = mix(ground, sky, n.y * 0.5 + 0.5) * cam.params.y;
    let fill = max(dot(n, normalize(vec3<f32>(-0.6, 0.35, -0.5))), 0.0) * 0.18;
    let lit = i.color * (ambient + diffuse * vec3<f32>(0.85, 0.82, 0.77) + fill)
        + vec3<f32>(specular) + i.emissive;
    var o: SceneOut;
    o.color = vec4<f32>(lit, 1.0);
    o.nd = vec4<f32>(n * 0.5 + 0.5, distance(i.world, cam.eye.xyz) * cam.params.x);
    return o;
}

struct Post {
    // xy: texel size of the source, zw: blur direction.
    texel_dir: vec4<f32>,
    // x: bloom threshold, y: bloom strength, z: edge strength, w: exposure.
    params: vec4<f32>,
};

@group(0) @binding(0) var t0: texture_2d<f32>;
@group(0) @binding(1) var t1: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> post: Post;

struct FullOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) index: u32) -> FullOut {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var o: FullOut;
    o.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

fn nd_at(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(t1));
    return textureLoad(t1, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

@fragment
fn fs_edges(i: FullOut) -> @location(0) vec4<f32> {
    let color = textureSample(t0, samp, i.uv).rgb;
    let size = vec2<f32>(textureDimensions(t1));
    let p = vec2<i32>(i.uv * size);
    let c = nd_at(p);
    var edge = 0.0;
    let offsets = array<vec2<i32>, 4>(vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1));
    for (var k = 0; k < 4; k = k + 1) {
        let o = nd_at(p + offsets[k]);
        let normal_step = 1.0 - dot(c.xyz * 2.0 - 1.0, o.xyz * 2.0 - 1.0);
        let depth_step = abs(c.w - o.w) * 60.0;
        let background = select(0.0, 1.0, (c.w == 0.0) != (o.w == 0.0));
        edge = max(edge, max(max(normal_step * 2.5, depth_step), background));
    }
    edge = clamp(edge, 0.0, 1.0) * post.params.z;
    let glow = vec3<f32>(0.30, 0.65, 0.95);
    return vec4<f32>(color + glow * edge * 0.55, 1.0);
}

@fragment
fn fs_bright(i: FullOut) -> @location(0) vec4<f32> {
    let t = post.texel_dir.xy;
    var c = vec3<f32>(0.0);
    c += textureSample(t0, samp, i.uv + vec2<f32>(-t.x, -t.y)).rgb;
    c += textureSample(t0, samp, i.uv + vec2<f32>(t.x, -t.y)).rgb;
    c += textureSample(t0, samp, i.uv + vec2<f32>(-t.x, t.y)).rgb;
    c += textureSample(t0, samp, i.uv + vec2<f32>(t.x, t.y)).rgb;
    c = c * 0.25;
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    let keep = max(luma - post.params.x, 0.0) / max(luma, 1e-4);
    return vec4<f32>(c * keep, 1.0);
}

@fragment
fn fs_blur(i: FullOut) -> @location(0) vec4<f32> {
    let step = post.texel_dir.xy * post.texel_dir.zw;
    let w = array<f32, 5>(0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);
    var c = textureSample(t0, samp, i.uv).rgb * w[0];
    for (var k = 1; k < 5; k = k + 1) {
        let o = step * f32(k) * 1.5;
        c += textureSample(t0, samp, i.uv + o).rgb * w[k];
        c += textureSample(t0, samp, i.uv - o).rgb * w[k];
    }
    return vec4<f32>(c, 1.0);
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_composite(i: FullOut) -> @location(0) vec4<f32> {
    let base = textureSample(t0, samp, i.uv).rgb;
    let bloom = textureSample(t1, samp, i.uv).rgb;
    let c = (base + bloom * post.params.y) * post.params.w;
    return vec4<f32>(aces(c), 1.0);
}
