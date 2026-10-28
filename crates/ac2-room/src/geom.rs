//! Plane geometry on plan coordinates (metres).

pub(crate) type P = [f64; 2];

/// Lengths below this are zero: plans are edited at 0.1 m, so a nanometre is far below any
/// meaningful difference and well above f64 rounding at room scale.
pub(crate) const EPS: f64 = 1e-9;

pub(crate) fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

pub(crate) fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

pub(crate) fn dist(a: P, b: P) -> f64 {
    let d = sub(a, b);
    d[0].hypot(d[1])
}

/// Twice the signed area of triangle o-a-b: positive when o→a→b turns left.
pub(crate) fn cross(o: P, a: P, b: P) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Signed area, positive for a counter-clockwise polygon.
pub(crate) fn signed_area(poly: &[P]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// Area centroid of a polygon with non-zero area.
pub(crate) fn centroid(poly: &[P]) -> P {
    let n = poly.len();
    let (mut cx, mut cy) = (0.0, 0.0);
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let w = a[0] * b[1] - b[0] * a[1];
        cx += (a[0] + b[0]) * w;
        cy += (a[1] + b[1]) * w;
    }
    let a6 = 6.0 * signed_area(poly);
    [cx / a6, cy / a6]
}

fn on_segment(p: P, a: P, b: P) -> bool {
    cross(a, b, p).abs() <= EPS * (1.0 + dist(a, b))
        && p[0] >= a[0].min(b[0]) - EPS
        && p[0] <= a[0].max(b[0]) + EPS
        && p[1] >= a[1].min(b[1]) - EPS
        && p[1] <= a[1].max(b[1]) + EPS
}

fn side(o: P, a: P, b: P) -> i8 {
    let c = cross(o, a, b);
    let tol = EPS * (1.0 + dist(o, a));
    if c > tol {
        1
    } else if c < -tol {
        -1
    } else {
        0
    }
}

/// Segments a-b and c-d share a point (touching counts).
pub(crate) fn segments_meet(a: P, b: P, c: P, d: P) -> bool {
    let (d1, d2) = (side(a, b, c), side(a, b, d));
    let (d3, d4) = (side(c, d, a), side(c, d, b));
    if d1 * d2 < 0 && d3 * d4 < 0 {
        return true;
    }
    on_segment(c, a, b) || on_segment(d, a, b) || on_segment(a, c, d) || on_segment(b, c, d)
}

/// Segments a-b and c-d cross at a point interior to both.
pub(crate) fn segments_cross(a: P, b: P, c: P, d: P) -> bool {
    side(a, b, c) * side(a, b, d) < 0 && side(c, d, a) * side(c, d, b) < 0
}

/// Inside or on the boundary.
pub(crate) fn inside(p: P, poly: &[P]) -> bool {
    let n = poly.len();
    let mut odd = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if on_segment(p, a, b) {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                odd = !odd;
            }
        }
    }
    odd
}

pub(crate) fn dist_to_segment(p: P, a: P, b: P) -> f64 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    if l2 <= EPS * EPS {
        return dist(p, a);
    }
    let t = (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + t * ab[0], a[1] + t * ab[1]])
}

/// Distance to the nearest edge of a polygon.
pub(crate) fn dist_to_boundary(p: P, poly: &[P]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| dist_to_segment(p, poly[i], poly[(i + 1) % n]))
        .fold(f64::INFINITY, f64::min)
}

/// The polygon without vertices that lie on the line through their neighbours: they do not
/// change its shape, so symmetry is judged without them.
pub(crate) fn without_collinear(poly: &[P]) -> Vec<P> {
    let n = poly.len();
    (0..n)
        .filter(|&i| {
            let (a, v, b) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            cross(a, v, b).abs() > EPS * (1.0 + dist(a, b))
        })
        .map(|i| poly[i])
        .collect()
}
