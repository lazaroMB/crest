//! Picking on the CPU: the ray under the cursor against the chips' boxes
//! of the same scene description, with no GPU readback.

use domain::sysmodel::scene::Scene;

use super::math::V3;

/// Distance along the ray to the box `min..max`, if it is hit.
#[must_use]
pub fn ray_box(origin: V3, dir: V3, min: V3, max: V3) -> Option<f32> {
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    for k in 0..3 {
        if dir[k].abs() < 1e-9 {
            if origin[k] < min[k] || origin[k] > max[k] {
                return None;
            }
            continue;
        }
        let (a, b) = ((min[k] - origin[k]) / dir[k], (max[k] - origin[k]) / dir[k]);
        near = near.max(a.min(b));
        far = far.min(a.max(b));
    }
    (near <= far && far >= 0.0).then_some(near.max(0.0))
}

/// The chip the ray hits first.
#[must_use]
pub fn pick(scene: &Scene, origin: V3, dir: V3) -> Option<&str> {
    scene
        .chips
        .iter()
        .filter_map(|c| ray_box(origin, dir, c.min(), c.max()).map(|t| (t, c.id.as_str())))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_hit_boxes_in_front() {
        let (min, max) = ([-1.0; 3], [1.0; 3]);
        assert_eq!(
            ray_box([0.0, 0.0, -5.0], [0.0, 0.0, 1.0], min, max),
            Some(4.0)
        );
        assert_eq!(ray_box([0.0, 0.0, -5.0], [0.0, 0.0, -1.0], min, max), None);
        assert_eq!(ray_box([3.0, 0.0, -5.0], [0.0, 0.0, 1.0], min, max), None);
        assert_eq!(ray_box([0.0; 3], [1.0, 0.0, 0.0], min, max), Some(0.0));
    }
}
