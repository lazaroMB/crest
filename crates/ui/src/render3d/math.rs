//! The little linear algebra the renderer needs: 3-vectors as `[f32; 3]`
//! and column-major 4×4 matrices (the layout WGSL expects), right-handed,
//! with depth in `0..=1` as wgpu clips it.

/// A 3-vector.
pub type V3 = [f32; 3];

/// A column-major 4×4 matrix: `m[col][row]`.
pub type M4 = [[f32; 4]; 4];

/// `a + b`.
#[must_use]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `a - b`.
#[must_use]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a * s`.
#[must_use]
pub fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Dot product.
#[must_use]
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product.
#[must_use]
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Length.
#[must_use]
pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}

/// `a` with length 1 (or `a` when it is zero).
#[must_use]
pub fn normalize(a: V3) -> V3 {
    let l = length(a);
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}

/// The identity.
#[must_use]
pub const fn identity() -> M4 {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// `a * b`.
#[must_use]
pub fn mul(a: &M4, b: &M4) -> M4 {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, cell) in col.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

/// `m * (p, 1)`, with the perspective divide.
#[must_use]
pub fn transform(m: &M4, p: V3) -> Option<V3> {
    let v = [p[0], p[1], p[2], 1.0];
    let mut o = [0.0f32; 4];
    for (r, cell) in o.iter_mut().enumerate() {
        *cell = (0..4).map(|k| m[k][r] * v[k]).sum();
    }
    (o[3].abs() > 1e-9).then(|| [o[0] / o[3], o[1] / o[3], o[2] / o[3]])
}

/// A view matrix looking from `eye` at `target`.
#[must_use]
pub fn look_at(eye: V3, target: V3, up: V3) -> M4 {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

/// Perspective projection, `fov_y` in radians, depth `0..=1`.
#[must_use]
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> M4 {
    let f = 1.0 / (fov_y / 2.0).tan();
    let range = far / (near - far);
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, range, -1.0],
        [0.0, 0.0, range * near, 0.0],
    ]
}

/// Orthographic projection of a `width × height` box, depth `0..=1`.
#[must_use]
pub fn orthographic(width: f32, height: f32, near: f32, far: f32) -> M4 {
    let range = 1.0 / (near - far);
    [
        [2.0 / width, 0.0, 0.0, 0.0],
        [0.0, 2.0 / height, 0.0, 0.0],
        [0.0, 0.0, range, 0.0],
        [0.0, 0.0, range * near, 1.0],
    ]
}

/// The inverse of `m`, if it has one.
#[must_use]
pub fn invert(m: &M4) -> Option<M4> {
    // Gauss-Jordan on the row-major copy.
    let mut a = [[0.0f64; 8]; 4];
    for r in 0..4 {
        for c in 0..4 {
            a[r][c] = f64::from(m[c][r]);
        }
        a[r][4 + r] = 1.0;
    }
    for col in 0..4 {
        let pivot = (col..4).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        let p = a[col][col];
        for v in &mut a[col] {
            *v /= p;
        }
        for r in 0..4 {
            if r != col {
                let f = a[r][col];
                let pivot_row = a[col];
                for (v, pv) in a[r].iter_mut().zip(pivot_row) {
                    *v -= f * pv;
                }
            }
        }
    }
    let mut out = [[0.0f32; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            #[allow(clippy::cast_possible_truncation)]
            {
                out[c][r] = a[r][4 + c] as f32;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    #[test]
    fn look_at_puts_the_target_in_front() {
        let v = look_at([0.0, 0.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0]);
        assert!(close(transform(&v, [0.0; 3]).unwrap(), [0.0, 0.0, -5.0]));
        assert!(close(
            transform(&v, [1.0, 0.0, 0.0]).unwrap(),
            [1.0, 0.0, -5.0]
        ));
    }

    #[test]
    fn projections_map_near_and_far_to_0_and_1() {
        let p = perspective(1.0, 1.0, 1.0, 10.0);
        assert!((transform(&p, [0.0, 0.0, -1.0]).unwrap()[2]).abs() < 1e-5);
        assert!((transform(&p, [0.0, 0.0, -10.0]).unwrap()[2] - 1.0).abs() < 1e-5);
        let o = orthographic(4.0, 2.0, 1.0, 10.0);
        assert!(close(
            transform(&o, [2.0, 1.0, -1.0]).unwrap(),
            [1.0, 1.0, 0.0]
        ));
        assert!((transform(&o, [0.0, 0.0, -10.0]).unwrap()[2] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn inverse_undoes_the_matrix() {
        let m = mul(
            &perspective(0.8, 1.5, 0.5, 50.0),
            &look_at([3.0, 4.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0]),
        );
        let inv = invert(&m).unwrap();
        let p = [0.3, -0.2, 0.7];
        let back = transform(&inv, transform(&m, p).unwrap()).unwrap();
        assert!(close(back, p), "{back:?}");
        assert!(invert(&[[0.0; 4]; 4]).is_none());
        assert_eq!(mul(&identity(), &m), m);
    }
}
