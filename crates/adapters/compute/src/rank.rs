//! Ranks with ties and the exact null distributions of the rank statistics.

/// Average ranks (1-based) of `values`, and the sizes of the tie groups
/// with more than one member.
pub fn ranks(values: &[f64]) -> (Vec<f64>, Vec<usize>) {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    let mut out = vec![0.0; values.len()];
    let mut ties = Vec::new();
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && values[order[j + 1]] == values[order[i]] {
            j += 1;
        }
        // Positions i..=j share the average of ranks i+1..=j+1.
        let average = (i + j) as f64 / 2.0 + 1.0;
        for &k in &order[i..=j] {
            out[k] = average;
        }
        if j > i {
            ties.push(j - i + 1);
        }
        i = j + 1;
    }
    (out, ties)
}

/// Σ (t³ − t) over tie groups, the tie correction of rank variances.
pub fn tie_sum(ties: &[usize]) -> f64 {
    ties.iter()
        .map(|&t| {
            let t = t as f64;
            t * t * t - t
        })
        .sum()
}

/// Counts of the Mann-Whitney U statistic for samples of `m` and `n`
/// without ties: `counts[u]` arrangements give `U = u`, for
/// `u = 0..=m·n`. Built from the subsets of size `m` of the ranks.
pub fn mann_whitney_counts(m: usize, n: usize) -> Vec<f64> {
    let total = m + n;
    let max_u = m * n;
    // dp[k][u]: subsets of size k of the first i items with "excess" u,
    // where the excess of a subset {r1 < … < rk} of 0-based ranks is
    // Σ (r_j − (j − 1)); U is the excess of the full subset.
    let mut dp = vec![vec![0.0f64; max_u + 1]; m + 1];
    dp[0][0] = 1.0;
    for item in 0..total {
        for k in (1..=m.min(item + 1)).rev() {
            // Adding `item` as the k-th smallest member adds item − (k − 1).
            let Some(shift) = item.checked_sub(k - 1) else {
                continue;
            };
            if shift > n {
                continue;
            }
            for u in (shift..=max_u).rev() {
                let add = dp[k - 1][u - shift];
                dp[k][u] += add;
            }
        }
    }
    dp.swap_remove(m)
}

/// Counts of the Wilcoxon signed-rank statistic V (sum of positive ranks)
/// for `n` untied, non-zero differences: `counts[v]`, `v = 0..=n(n+1)/2`.
pub fn signed_rank_counts(n: usize) -> Vec<f64> {
    let max = n * (n + 1) / 2;
    let mut dp = vec![0.0f64; max + 1];
    dp[0] = 1.0;
    for rank in 1..=n {
        for v in (rank..=max).rev() {
            dp[v] += dp[v - rank];
        }
    }
    dp
}

/// Counts of permutations of `n` items by number of inversions (Mahonian
/// numbers), `counts[k]`, `k = 0..=n(n−1)/2`: the null distribution of
/// Kendall's discordant pairs without ties.
pub fn inversion_counts(n: usize) -> Vec<f64> {
    let max = n * n.saturating_sub(1) / 2;
    let mut dp = vec![0.0f64; max + 1];
    dp[0] = 1.0;
    for i in 1..n {
        // Inserting the (i+1)-th item adds 0..=i inversions.
        let mut next = vec![0.0f64; max + 1];
        let mut window = 0.0;
        for k in 0..=max {
            window += dp[k];
            if k > i {
                window -= dp[k - i - 1];
            }
            next[k] = window;
        }
        dp = next;
    }
    dp
}

/// P(X ≤ x) and P(X ≥ x) of a discrete distribution given by counts.
pub fn tails(counts: &[f64], x: usize) -> (f64, f64) {
    let total: f64 = counts.iter().sum();
    let lower: f64 = counts.iter().take(x + 1).sum();
    let upper: f64 = counts.iter().skip(x).sum();
    (lower / total, upper / total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_average_ties() {
        let (r, ties) = ranks(&[3.0, 1.0, 3.0, 2.0]);
        assert_eq!(r, vec![3.5, 1.0, 3.5, 2.0]);
        assert_eq!(ties, vec![2]);
        assert_eq!(tie_sum(&ties), 6.0);
    }

    #[test]
    fn exact_distributions_have_the_right_totals() {
        // C(5, 2) = 10 arrangements, U symmetric on 0..=6.
        let mw = mann_whitney_counts(2, 3);
        assert_eq!(mw, vec![1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0]);
        let sr = signed_rank_counts(3);
        assert_eq!(sr, vec![1.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0]);
        let inv = inversion_counts(3);
        assert_eq!(inv, vec![1.0, 2.0, 2.0, 1.0]);
        assert_eq!(tails(&inv, 1), (0.5, 5.0 / 6.0));
    }
}
