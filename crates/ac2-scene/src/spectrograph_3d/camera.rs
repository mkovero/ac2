//! The 3D spectrograph camera: pose, clamps, the fit of the floor box at every angle and
//! the projection.

use ac2_proto::model::layout::SpectrumCamera;

use super::*;

/// The camera as drawn: continuous, so a key step can ease and a drag follow the pointer;
/// stored as a [`SpectrumCamera`] in whole degrees and steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub yaw_deg: f64,
    pub pitch_deg: f64,
    pub zoom_step: f64,
}

impl From<SpectrumCamera> for Pose {
    fn from(c: SpectrumCamera) -> Self {
        Self {
            yaw_deg: f64::from(c.yaw_deg),
            pitch_deg: f64::from(c.pitch_deg),
            zoom_step: f64::from(c.zoom_step),
        }
    }
}

impl Pose {
    /// Within the camera's ranges.
    pub fn clamped(self) -> Self {
        let yaw = f64::from(SpectrumCamera::YAW_MAX_DEG);
        let p = SpectrumCamera::PITCH_DEG;
        let z = SpectrumCamera::ZOOM_STEPS;
        Self {
            yaw_deg: self.yaw_deg.clamp(-yaw, yaw),
            pitch_deg: self
                .pitch_deg
                .clamp(f64::from(*p.start()), f64::from(*p.end())),
            zoom_step: self
                .zoom_step
                .clamp(f64::from(*z.start()), f64::from(*z.end())),
        }
    }

    /// The nearest stored camera: whole degrees and steps, within the ranges.
    pub fn snapped(self) -> SpectrumCamera {
        let c = self.clamped();
        SpectrumCamera {
            yaw_deg: c.yaw_deg.round() as i8,
            pitch_deg: c.pitch_deg.round() as u8,
            zoom_step: c.zoom_step.round() as i8,
        }
    }

    /// Unit vector from the orbit target towards the eye.
    fn towards_eye(self) -> V3 {
        let (y, p) = (self.yaw_deg.to_radians(), self.pitch_deg.to_radians());
        [y.sin() * p.cos(), p.sin(), -y.cos() * p.cos()]
    }
}

/// `c` moved by key steps: `yaw` steps of [`YAW_KEY_DEG`] (positive: the eye moves right),
/// `pitch` of [`PITCH_KEY_DEG`] (positive: up), `zoom` steps (positive: closer); clamped.
pub fn step(c: SpectrumCamera, yaw: i32, pitch: i32, zoom: i32) -> SpectrumCamera {
    let yaw_max = i32::from(SpectrumCamera::YAW_MAX_DEG);
    let p = SpectrumCamera::PITCH_DEG;
    let z = SpectrumCamera::ZOOM_STEPS;
    SpectrumCamera {
        yaw_deg: (i32::from(c.yaw_deg) + yaw * YAW_KEY_DEG).clamp(-yaw_max, yaw_max) as i8,
        pitch_deg: (i32::from(c.pitch_deg) + pitch * PITCH_KEY_DEG)
            .clamp(i32::from(*p.start()), i32::from(*p.end())) as u8,
        zoom_step: (i32::from(c.zoom_step) + zoom).clamp(i32::from(*z.start()), i32::from(*z.end()))
            as i8,
    }
}

/// `c` moved by a drag of `dx`, `dy` logical pixels (right, down): the eye orbits with the
/// pointer, a drag up looks from higher. Continuous; snapped on release.
pub fn drag(c: Pose, dx: f64, dy: f64) -> Pose {
    Pose {
        yaw_deg: c.yaw_deg - dx * DRAG_YAW_DEG_PER_PX,
        pitch_deg: c.pitch_deg - dy * DRAG_PITCH_DEG_PER_PX,
        ..c
    }
    .clamped()
}

pub(super) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(super) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(super) fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(super) fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(super) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(super) fn normalize(a: V3) -> V3 {
    scale(a, 1.0 / dot(a, a).sqrt())
}

/// The skyline's box: the floor and the full height above it.
pub(super) const BOX_MIN: V3 = [-FLOOR_HALF_W, 0.0, 0.0];
pub(super) const BOX_MAX: V3 = [FLOOR_HALF_W, MAX_HEIGHT, FLOOR_DEPTH];

pub(super) fn box_corners() -> [V3; 8] {
    let mut out = [[0.0; 3]; 8];
    for (i, c) in out.iter_mut().enumerate() {
        for (a, v) in c.iter_mut().enumerate() {
            *v = if i & (1 << a) != 0 {
                BOX_MAX[a]
            } else {
                BOX_MIN[a]
            };
        }
    }
    out
}

/// Distance along `dir` (unit) from the target to where the ray leaves the box.
pub(super) fn exit_distance(dir: V3) -> f64 {
    (0..3)
        .filter(|&a| dir[a] != 0.0)
        .map(|a| {
            let bound = if dir[a] > 0.0 { BOX_MAX[a] } else { BOX_MIN[a] };
            (bound - TARGET[a]) / dir[a]
        })
        .fold(f64::INFINITY, f64::min)
}

/// World to clip space for a plot of aspect `aspect` (width ÷ height).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewProj {
    /// Row-major: `clip[i] = Σ m[i][j] · p[j]`, `p = (x, y, z, 1)`.
    pub m: [[f64; 4]; 4],
    pub eye: V3,
}

impl ViewProj {
    pub fn clip(&self, p: V3) -> [f64; 4] {
        let v = [p[0], p[1], p[2], 1.0];
        let row = |i: usize| (0..4).map(|j| self.m[i][j] * v[j]).sum::<f64>();
        [row(0), row(1), row(2), row(3)]
    }

    /// Normalised device coordinates, `None` behind the near plane.
    pub fn ndc(&self, p: V3) -> Option<[f64; 2]> {
        let c = self.clip(p);
        (c[3] >= NEAR).then(|| [c[0] / c[3], c[1] / c[3]])
    }

    /// Column-major `f32` for the GPU.
    pub fn gpu(&self) -> [[f32; 4]; 4] {
        let mut out = [[0.0; 4]; 4];
        for (j, col) in out.iter_mut().enumerate() {
            for (i, v) in col.iter_mut().enumerate() {
                *v = self.m[i][j] as f32;
            }
        }
        out
    }
}

/// `shift` moves the picture up the plot by that much of its half-height (an off-axis
/// lens: the pose and its perspective stay, only the frame slides).
pub(super) fn view_proj_at(pose: Pose, distance: f64, aspect: f64, shift: f64) -> ViewProj {
    let eye = add(TARGET, scale(pose.towards_eye(), distance));
    let f = normalize(sub(TARGET, eye));
    // Right-handed world seen with +x to the right of the screen when looking along +z:
    // right = up × forward.
    let r = normalize(cross([0.0, 1.0, 0.0], f));
    let u = cross(f, r);
    let fy = 1.0 / (FOV_Y_DEG.to_radians() / 2.0).tan();
    let fx = fy / aspect;
    let d = FAR / (FAR - NEAR);
    let row = |a: V3, k: f64, off: f64| [a[0] * k, a[1] * k, a[2] * k, -dot(a, eye) * k + off];
    ViewProj {
        m: [
            row(r, fx, 0.0),
            row(add(scale(u, fy), scale(f, shift)), 1.0, 0.0),
            row(f, d, -NEAR * d),
            row(f, 1.0, 0.0),
        ],
        eye,
    }
}

/// The box's lowest and highest corner in NDC y at `distance`, unshifted; `None` when a
/// corner is behind the near plane.
pub(super) fn box_y_extent(pose: Pose, distance: f64, aspect: f64) -> Option<(f64, f64)> {
    let vp = view_proj_at(pose, distance, aspect, 0.0);
    box_corners()
        .iter()
        .try_fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            let n = vp.ndc(*p)?;
            Some((lo.min(n[1]), hi.max(n[1])))
        })
}

/// The distance at zoom step 0 for `pose`'s angles in a plot of aspect `aspect`: every
/// corner of the box at full height fits with a 7 % margin, the box centred up and down
/// ([`lens_shift`]), so the front edge's tall spikes are never cut at the sides and a
/// steep or turned camera keeps the near floor corner in the picture. Fitted per angle, not
/// once at the default: from above and aside the box's projected outline is another shape.
/// The fit is continuous in the angles (each corner's distance to fit is), so a camera
/// easing through an orbit dollies smoothly with it.
pub fn fitted_distance(pose: Pose, aspect: f64) -> f64 {
    let pose = pose.clamped();
    let lim = 1.0 - FIT_MARGIN;
    let fits = |d: f64| {
        let vp = view_proj_at(pose, d, aspect, 0.0);
        box_corners()
            .iter()
            .all(|p| vp.ndc(*p).is_some_and(|n| n[0].abs() <= lim))
            && box_y_extent(pose, d, aspect).is_some_and(|(lo, hi)| {
                // The lens shift centres the box but moves it at most half the plot.
                let shift = (-(lo + hi) / 2.0).clamp(-SHIFT_MAX, SHIFT_MAX);
                lo + shift >= -lim && hi + shift <= lim
            })
    };
    let (mut lo, mut hi) = (0.1, 100.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// The lens shift's bound, in NDC: half the plot.
pub(super) const SHIFT_MAX: f64 = 0.5;

/// The lens shift that centres the box up and down in the plot for `pose`'s angles: a low
/// camera sees the floor as a thin strip under tall spikes, and with the orbit target in
/// the middle of the floor the picture would sit high, a dead band under it. Taken at the
/// zoom-0 distance, so a dolly scales the picture about the same point.
pub fn lens_shift(pose: Pose, aspect: f64) -> f64 {
    let pose = pose.clamped();
    let d = eye_distance(
        Pose {
            zoom_step: 0.0,
            ..pose
        },
        aspect,
    );
    box_y_extent(pose, d, aspect).map_or(0.0, |(lo, hi)| {
        (-(lo + hi) / 2.0).clamp(-SHIFT_MAX, SHIFT_MAX)
    })
}

/// The eye's distance from the target: the dolly's, never inside the grown box. The stored
/// zoom step is left as set; only the distance drawn is held off.
pub fn eye_distance(pose: Pose, aspect: f64) -> f64 {
    let pose = pose.clamped();
    let dolly = fitted_distance(pose, aspect) * 2f64.powf(-pose.zoom_step / ZOOM_STEPS_PER_OCTAVE);
    dolly.max(exit_distance(pose.towards_eye()) + EYE_CLEARANCE)
}

/// The camera's view-projection for a plot of aspect `aspect`.
pub fn view_proj(pose: Pose, aspect: f64) -> ViewProj {
    let pose = pose.clamped();
    view_proj_at(
        pose,
        eye_distance(pose, aspect),
        aspect,
        lens_shift(pose, aspect),
    )
}

/// Whether `eye` is outside the skyline's box grown by `margin`.
pub fn outside_box(eye: V3, margin: f64) -> bool {
    (0..3).any(|a| eye[a] < BOX_MIN[a] - margin || eye[a] > BOX_MAX[a] + margin)
}
