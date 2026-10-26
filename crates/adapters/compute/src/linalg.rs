//! Small dense linear algebra for OLS: Householder QR and singular values.
//!
//! Matrices are column-major `Vec<Vec<f64>>` (one `Vec` per column), the
//! natural shape of a design matrix built from data columns. Least squares
//! goes through QR, never the normal equations, so ill-conditioned designs
//! (NIST Longley) keep their digits.

/// A Householder QR factorization of an `n × k` matrix, `n ≥ k`.
pub struct Qr {
    /// Reflector vectors; `v[j]` is zero above row `j`.
    v: Vec<Vec<f64>>,
    /// Reflector coefficients (`H = I − β v vᵀ`).
    beta: Vec<f64>,
    /// The upper triangle, `r[i][j]` for `j ≥ i` (row-major).
    r: Vec<Vec<f64>>,
    n: usize,
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm(a: &[f64]) -> f64 {
    // Scaled to avoid overflow on large columns.
    let scale = a.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    if scale == 0.0 {
        return 0.0;
    }
    scale * a.iter().map(|x| (x / scale).powi(2)).sum::<f64>().sqrt()
}

impl Qr {
    /// Factorizes `columns` (each of length `n`).
    pub fn new(columns: &[Vec<f64>]) -> Self {
        let k = columns.len();
        let n = columns.first().map_or(0, Vec::len);
        let mut a: Vec<Vec<f64>> = columns.to_vec();
        let mut v = Vec::with_capacity(k);
        let mut beta = Vec::with_capacity(k);
        let mut r = vec![vec![0.0; k]; k];
        for j in 0..k {
            let alpha = norm(&a[j][j..]);
            let sign = if a[j][j] >= 0.0 { 1.0 } else { -1.0 };
            let mut vj = vec![0.0; n];
            vj[j..].copy_from_slice(&a[j][j..]);
            vj[j] += sign * alpha;
            let vnorm2 = dot(&vj[j..], &vj[j..]);
            let bj = if vnorm2 > 0.0 { 2.0 / vnorm2 } else { 0.0 };
            for col in a.iter_mut().skip(j) {
                let s = bj * dot(&vj[j..], &col[j..]);
                for (c, x) in col[j..].iter_mut().zip(&vj[j..]) {
                    *c -= s * x;
                }
            }
            for (i, row) in r.iter_mut().enumerate().take(j + 1) {
                row[j] = a[j][i];
            }
            v.push(vj);
            beta.push(bj);
        }
        Self { v, beta, r, n }
    }

    /// The diagonal of R.
    pub fn r_diag(&self) -> Vec<f64> {
        self.r.iter().enumerate().map(|(i, row)| row[i]).collect()
    }

    /// Qᵀ y.
    pub fn qt_mul(&self, y: &[f64]) -> Vec<f64> {
        let mut y = y.to_vec();
        for (vj, bj) in self.v.iter().zip(&self.beta) {
            let s = bj * dot(vj, &y);
            for (c, x) in y.iter_mut().zip(vj) {
                *c -= s * x;
            }
        }
        y
    }

    /// Solves R b = (Qᵀ y)[..k], the least-squares coefficients.
    pub fn solve(&self, y: &[f64]) -> Vec<f64> {
        let k = self.r.len();
        let qty = self.qt_mul(y);
        let mut b = vec![0.0; k];
        for i in (0..k).rev() {
            let s: f64 = (i + 1..k).map(|j| self.r[i][j] * b[j]).sum();
            b[i] = (qty[i] - s) / self.r[i][i];
        }
        b
    }

    /// (RᵀR)⁻¹ = (AᵀA)⁻¹, row-major `k × k`.
    pub fn inverse_gram(&self) -> Vec<Vec<f64>> {
        let k = self.r.len();
        // R⁻¹ by back substitution, column by column.
        let mut rinv = vec![vec![0.0; k]; k];
        #[allow(clippy::needless_range_loop)]
        for col in 0..k {
            for i in (0..=col).rev() {
                let rhs = if i == col { 1.0 } else { 0.0 };
                let s: f64 = (i + 1..=col).map(|j| self.r[i][j] * rinv[j][col]).sum();
                rinv[i][col] = (rhs - s) / self.r[i][i];
            }
        }
        let mut out = vec![vec![0.0; k]; k];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..k).map(|m| rinv[i][m] * rinv[j][m]).sum();
            }
        }
        out
    }

    /// Squared row norms of the thin Q: the leverages of the factored
    /// columns.
    pub fn leverages(&self) -> Vec<f64> {
        let k = self.r.len();
        let mut h = vec![0.0; self.n];
        for j in 0..k {
            // Q e_j: apply the reflectors in reverse to the unit vector.
            let mut e = vec![0.0; self.n];
            e[j] = 1.0;
            for (vi, bi) in self.v.iter().zip(&self.beta).rev() {
                let s = bi * dot(vi, &e);
                for (c, x) in e.iter_mut().zip(vi) {
                    *c -= s * x;
                }
            }
            for (hi, qi) in h.iter_mut().zip(&e) {
                *hi += qi * qi;
            }
        }
        h
    }
}

/// Singular values of `columns` by one-sided Jacobi (Hestenes), which is
/// accurate for the small ones too; descending.
pub fn singular_values(columns: &[Vec<f64>]) -> Vec<f64> {
    let mut a: Vec<Vec<f64>> = columns.to_vec();
    let k = a.len();
    for _sweep in 0..100 {
        let mut rotated = false;
        for i in 0..k {
            for j in i + 1..k {
                let alpha = dot(&a[i], &a[i]);
                let beta = dot(&a[j], &a[j]);
                let gamma = dot(&a[i], &a[j]);
                if gamma.abs() <= 1e-15 * (alpha * beta).sqrt() || gamma == 0.0 {
                    continue;
                }
                rotated = true;
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                let (left, right) = a.split_at_mut(j);
                for (x, y) in left[i].iter_mut().zip(right[0].iter_mut()) {
                    let (xi, yi) = (*x, *y);
                    *x = c * xi - s * yi;
                    *y = s * xi + c * yi;
                }
            }
        }
        if !rotated {
            break;
        }
    }
    let mut s: Vec<f64> = a.iter().map(|c| norm(c)).collect();
    s.sort_by(|x, y| y.total_cmp(x));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_solves_least_squares_and_inverts_the_gram_matrix() {
        // y = 1 + 2x exactly.
        let x = vec![1.0, 1.0, 1.0, 1.0];
        let t = vec![0.0, 1.0, 2.0, 3.0];
        let y: Vec<f64> = t.iter().map(|t| 1.0 + 2.0 * t).collect();
        let qr = Qr::new(&[x, t]);
        let b = qr.solve(&y);
        assert!((b[0] - 1.0).abs() < 1e-12 && (b[1] - 2.0).abs() < 1e-12);
        // AᵀA = [[4, 6], [6, 14]], inverse = [[0.7, -0.3], [-0.3, 0.2]].
        let inv = qr.inverse_gram();
        assert!((inv[0][0] - 0.7).abs() < 1e-12);
        assert!((inv[0][1] + 0.3).abs() < 1e-12);
        assert!((inv[1][1] - 0.2).abs() < 1e-12);
        let h: f64 = qr.leverages().iter().sum();
        assert!((h - 2.0).abs() < 1e-12);
    }

    #[test]
    fn singular_values_of_a_diagonal_matrix() {
        let s = singular_values(&[vec![3.0, 0.0], vec![0.0, 4.0]]);
        assert!((s[0] - 4.0).abs() < 1e-12 && (s[1] - 3.0).abs() < 1e-12);
    }
}
