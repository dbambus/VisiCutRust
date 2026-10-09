//! 2D affine maps, path building with exact Bézier arcs and bounding boxes.
use std::f64::consts::{FRAC_PI_2, TAU};

pub type P = [f64; 2];

/// `x' = a·x + c·y + e`, `y' = b·x + d·y + f` (SVG matrix order).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(x: f64, y: f64) -> Affine {
        Affine {
            e: x,
            f: y,
            ..Self::IDENTITY
        }
    }

    pub fn scale(x: f64, y: f64) -> Affine {
        Affine {
            a: x,
            d: y,
            ..Self::IDENTITY
        }
    }

    pub fn rotate(radians: f64) -> Affine {
        let (sin, cos) = radians.sin_cos();
        Affine {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Shears x by the oblique angle (positive leans to the right).
    pub fn skew_x(radians: f64) -> Affine {
        Affine {
            c: radians.tan(),
            ..Self::IDENTITY
        }
    }

    /// `self ∘ other`: applies `other` first.
    pub fn then(self, other: Affine) -> Affine {
        Affine {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn apply(&self, p: P) -> P {
        [
            self.a * p[0] + self.c * p[1] + self.e,
            self.b * p[0] + self.d * p[1] + self.f,
        ]
    }

    /// Geometric mean scale factor.
    pub fn scale_factor(&self) -> f64 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }

    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f]
            .iter()
            .all(|v| v.is_finite())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(P),
    Line(P),
    Cubic(P, P, P),
    Close,
}

/// Collects segments in world coordinates; inputs are local coordinates
/// mapped through `m`.
pub struct PathBuilder {
    pub m: Affine,
    pub segs: Vec<Seg>,
    current: Option<P>,
    start: P,
}

/// Chord tolerance (mm) for curves that are flattened.
const TOLERANCE: f64 = 0.005;

fn close_to(a: P, b: P) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

impl PathBuilder {
    pub fn new(m: Affine) -> Self {
        PathBuilder {
            m,
            segs: Vec::new(),
            current: None,
            start: [0.0; 2],
        }
    }

    pub fn move_to(&mut self, p: P) {
        let p = self.m.apply(p);
        if let Some(Seg::Move(_)) = self.segs.last() {
            self.segs.pop();
        }
        self.segs.push(Seg::Move(p));
        self.current = Some(p);
        self.start = p;
    }

    /// Makes the next drawing command start a new subpath.
    pub fn break_path(&mut self) {
        self.current = None;
    }

    /// Whether anything besides moves was drawn.
    pub fn has_drawing(&self) -> bool {
        self.segs.iter().any(|s| !matches!(s, Seg::Move(_)))
    }

    /// Continues the open subpath at `p`, connecting with a line if needed.
    pub fn connect(&mut self, p: P) {
        match self.current {
            None => self.move_to(p),
            Some(current) => {
                let world = self.m.apply(p);
                if !close_to(current, world) {
                    self.segs.push(Seg::Line(world));
                    self.current = Some(world);
                }
            }
        }
    }

    pub fn line_to(&mut self, p: P) {
        if self.current.is_none() {
            return self.move_to(p);
        }
        let p = self.m.apply(p);
        self.segs.push(Seg::Line(p));
        self.current = Some(p);
    }

    fn line_to_world(&mut self, p: P) {
        self.segs.push(Seg::Line(p));
        self.current = Some(p);
    }

    pub fn cubic_to(&mut self, c1: P, c2: P, p: P) {
        let p = self.m.apply(p);
        self.segs
            .push(Seg::Cubic(self.m.apply(c1), self.m.apply(c2), p));
        self.current = Some(p);
    }

    pub fn close(&mut self) {
        if self.current.is_some() {
            if let Some(Seg::Line(p)) = self.segs.last()
                && close_to(*p, self.start)
            {
                self.segs.pop();
            }
            self.segs.push(Seg::Close);
            self.current = Some(self.start);
        }
    }

    /// Elliptical arc `center + major·cos t + minor·sin t` from `t0` over
    /// `sweep` radians, appended to the current subpath.
    pub fn arc(&mut self, center: P, major: P, minor: P, t0: f64, sweep: f64) {
        let point = |t: f64| {
            let (s, c) = t.sin_cos();
            [
                center[0] + major[0] * c + minor[0] * s,
                center[1] + major[1] * c + minor[1] * s,
            ]
        };
        let tangent = |t: f64| {
            let (s, c) = t.sin_cos();
            [-major[0] * s + minor[0] * c, -major[1] * s + minor[1] * c]
        };
        self.connect(point(t0));
        let n = (sweep.abs() / FRAC_PI_2 - 1e-9).ceil().max(1.0) as usize;
        let step = sweep / n as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        for i in 0..n {
            let a = t0 + step * i as f64;
            let b = a + step;
            let (pa, pb, ta, tb) = (point(a), point(b), tangent(a), tangent(b));
            self.cubic_to(
                [pa[0] + k * ta[0], pa[1] + k * ta[1]],
                [pb[0] - k * tb[0], pb[1] - k * tb[1]],
                pb,
            );
        }
    }

    /// Circular arc from `from` to `to` with a DXF bulge (tan of a quarter of
    /// the included angle, positive counter-clockwise).
    pub fn bulge_to(&mut self, from: P, to: P, bulge: f64) {
        let chord = [to[0] - from[0], to[1] - from[1]];
        let length = chord[0].hypot(chord[1]);
        if bulge.abs() < 1e-9 || length < 1e-12 || !bulge.is_finite() {
            return self.line_to(to);
        }
        let angle = 4.0 * bulge.atan();
        let offset = length / 2.0 / (angle / 2.0).tan();
        let normal = [-chord[1] / length, chord[0] / length];
        let center = [
            (from[0] + to[0]) / 2.0 + normal[0] * offset,
            (from[1] + to[1]) / 2.0 + normal[1] * offset,
        ];
        let radius = (from[0] - center[0]).hypot(from[1] - center[1]);
        let start = (from[1] - center[1]).atan2(from[0] - center[0]);
        self.arc(center, [radius, 0.0], [0.0, radius], start, angle);
        // Land exactly on the next vertex.
        if let Some(Seg::Cubic(_, _, end)) = self.segs.last_mut() {
            *end = self.m.apply(to);
            self.current = Some(*end);
        }
    }

    /// Flattens a parametric curve given in local coordinates over the
    /// parameter intervals `knots` (each split adaptively).
    pub fn curve(&mut self, f: &dyn Fn(f64) -> P, knots: &[f64]) {
        let Some(first) = knots.first() else { return };
        let m = self.m;
        let world = |t: f64| m.apply(f(t));
        self.connect(f(*first));
        for pair in knots.windows(2) {
            let (t0, t1) = (pair[0], pair[1]);
            if t1 <= t0 {
                continue;
            }
            for i in 0..4 {
                let a = t0 + (t1 - t0) * i as f64 / 4.0;
                let b = t0 + (t1 - t0) * (i + 1) as f64 / 4.0;
                self.subdivide(&world, a, world(a), b, world(b), 0);
            }
        }
    }

    fn subdivide(&mut self, f: &dyn Fn(f64) -> P, t0: f64, p0: P, t1: f64, p1: P, depth: u32) {
        let tm = (t0 + t1) / 2.0;
        let pm = f(tm);
        let chord = [p1[0] - p0[0], p1[1] - p0[1]];
        let length = chord[0].hypot(chord[1]);
        let deviation = if length < 1e-12 {
            (pm[0] - p0[0]).hypot(pm[1] - p0[1])
        } else {
            ((pm[0] - p0[0]) * chord[1] - (pm[1] - p0[1]) * chord[0]).abs() / length
        };
        if depth < 12 && (deviation > TOLERANCE || depth < 1) {
            self.subdivide(f, t0, p0, tm, pm, depth + 1);
            self.subdivide(f, tm, pm, t1, p1, depth + 1);
        } else {
            self.line_to_world(p1);
        }
    }
}

pub fn full_turn(sweep: f64) -> bool {
    (sweep.abs() - TAU).abs() < 1e-9
}

#[derive(Clone, Copy, Debug)]
pub struct BBox {
    pub min: P,
    pub max: P,
}

impl Default for BBox {
    fn default() -> Self {
        BBox {
            min: [f64::INFINITY; 2],
            max: [f64::NEG_INFINITY; 2],
        }
    }
}

impl BBox {
    pub fn add(&mut self, p: P) {
        for (i, value) in p.into_iter().enumerate() {
            self.min[i] = self.min[i].min(value);
            self.max[i] = self.max[i].max(value);
        }
    }

    pub fn is_empty(&self) -> bool {
        !(self.min[0] <= self.max[0] && self.min[1] <= self.max[1])
    }

    pub fn add_segments(&mut self, segs: &[Seg]) {
        let mut current = [0.0; 2];
        for seg in segs {
            match *seg {
                Seg::Move(p) | Seg::Line(p) => {
                    self.add(p);
                    current = p;
                }
                Seg::Cubic(c1, c2, p) => {
                    self.add(p);
                    for axis in 0..2 {
                        for t in cubic_extrema(current[axis], c1[axis], c2[axis], p[axis]) {
                            self.add(cubic_point(current, c1, c2, p, t));
                        }
                    }
                    current = p;
                }
                Seg::Close => {}
            }
        }
    }
}

fn cubic_point(p0: P, p1: P, p2: P, p3: P, t: f64) -> P {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    [0, 1].map(|i| w[0] * p0[i] + w[1] * p1[i] + w[2] * p2[i] + w[3] * p3[i])
}

/// Parameters in (0, 1) where a cubic's derivative is zero along one axis.
fn cubic_extrema(p0: f64, p1: f64, p2: f64, p3: f64) -> Vec<f64> {
    // Derivative / 3: a t² + b t + c.
    let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    let b = 2.0 * (p0 - 2.0 * p1 + p2);
    let c = p1 - p0;
    let mut roots = Vec::new();
    if a.abs() < 1e-12 {
        if b.abs() > 1e-12 {
            roots.push(-c / b);
        }
    } else {
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 {
            let s = disc.sqrt();
            roots.push((-b + s) / (2.0 * a));
            roots.push((-b - s) / (2.0 * a));
        }
    }
    roots.retain(|t| *t > 0.0 && *t < 1.0);
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulge_semicircle_has_tight_bounds() {
        let mut path = PathBuilder::new(Affine::IDENTITY);
        path.move_to([0.0, 0.0]);
        path.bulge_to([0.0, 0.0], [10.0, 0.0], 1.0);
        let mut bbox = BBox::default();
        bbox.add_segments(&path.segs);
        // Counter-clockwise from (0,0) to (10,0) bows below the chord.
        assert!((bbox.min[1] + 5.0).abs() < 1e-3, "{bbox:?}");
        assert!(bbox.max[1].abs() < 1e-9);
        assert!((bbox.max[0] - 10.0).abs() < 1e-9);
    }
}
