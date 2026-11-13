//! numpy's percentile and median on f32 data, bit for bit.
//!
//! `np.percentile(a, q)` with a Python-float q ("linear", Hyndman & Fan 7):
//! virtual index `v = (n - 1) * (q / 100)` in f64, `g = v - floor(v)`; then in
//! f32, `lo + (hi - lo) * f32(g)`, or `hi - (hi - lo) * f32(1 - g)` when
//! g >= 0.5. `np.median`: the middle value, or the f32 mean of the two.

/// The k-th smallest (0-based); reorders `v`.
fn kth(v: &mut [f32], k: usize) -> f32 {
    *v.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1
}

/// `np.percentile(v, q)` for f32 data; reorders `v`. NaN for empty input.
pub fn percentile(v: &mut [f32], q: f64) -> f32 {
    let n = v.len();
    if n == 0 {
        return f32::NAN;
    }
    let vi = (n - 1) as f64 * (q / 100.0);
    let i = vi.floor() as usize;
    let g = vi - i as f64;
    let lo = kth(v, i);
    let hi = if i + 1 < n {
        // the smallest of the upper part after the partition
        v[i + 1..].iter().copied().fold(f32::INFINITY, f32::min)
    } else {
        lo
    };
    let d = hi - lo;
    if g >= 0.5 { hi - d * (1.0 - g) as f32 } else { lo + d * g as f32 }
}

/// `np.median(v)` for f32 data; reorders `v`.
pub fn median(v: &mut [f32]) -> f32 {
    let n = v.len();
    if n == 0 {
        return f32::NAN;
    }
    let hi = kth(v, n / 2);
    if n % 2 == 1 {
        return hi;
    }
    let lo = v[..n / 2].iter().copied().fold(f32::NEG_INFINITY, f32::max);
    (lo + hi) / 2.0
}

/// numpy's f32 mean of three values (`a.mean()` on a length-3 axis).
#[inline]
pub fn mean3(v: [f32; 3]) -> f32 {
    (v[0] + v[1] + v[2]) / 3.0
}


/// numpy's float32 pairwise sum (`pairwise_sum_FLOAT`, block 128, 8-way
/// unrolled).
pub fn pairwise_sum(a: &[f32]) -> f32 {
    let n = a.len();
    if n < 8 {
        let mut r = 0.0f32;
        for &v in a {
            r += v;
        }
        r
    } else if n <= 128 {
        let mut r = [0f32; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i < n - n % 8 {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

/// numpy's `add.reduce` of a contiguous f32 run (`a.sum()`): the pairwise
/// sum of the whole run.
pub fn np_sum(a: &[f32]) -> f32 {
    pairwise_sum(a)
}

/// numpy's f32 reduction over the last two axes of a strided block
/// (`x.sum(axis=(2, 3))` on a tiled view): 0, plus the pairwise sum of each
/// inner row in turn.
pub fn block_sum<'a>(rows: impl Iterator<Item = &'a [f32]>) -> f32 {
    let mut out = 0.0f32;
    for r in rows {
        out += pairwise_sum(r);
    }
    out
}

/// numpy's f64 pairwise sum (`pairwise_sum_DOUBLE`).
pub fn pairwise_sum_f64(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        let mut r = 0.0f64;
        for &v in a {
            r += v;
        }
        r
    } else if n <= 128 {
        let mut r = [0f64; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i < n - n % 8 {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum_f64(&a[..n2]) + pairwise_sum_f64(&a[n2..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_cases() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), 2.5);
        assert_eq!(percentile(&mut [1.0, 2.0, 3.0, 4.0, 5.0], 50.0), 3.0);
        assert_eq!(percentile(&mut [0.0, 10.0], 25.0), 2.5);
        assert_eq!(percentile(&mut [0.0, 10.0], 75.0), 7.5);
        assert_eq!(percentile(&mut [7.0], 99.5), 7.0);
    }
}
