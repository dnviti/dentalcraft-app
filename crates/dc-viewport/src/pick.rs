//! CPU picking: nearest hit of a world ray against transformed meshes.

use dc_mesh::TriMesh;
use glam::{Mat3, Mat4, Vec3};

use crate::MeshId;
use crate::bvh::Bvh;
use crate::camera::Ray;

/// Result of a pick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pick {
    /// Id of the picked mesh ([`crate::ViewMesh::id`]).
    pub mesh_id: MeshId,
    /// Triangle index in the mesh.
    pub tri: u32,
    /// Hit point (world, mm).
    pub point: Vec3,
    /// Unit face normal (world), as stored by the triangle winding.
    pub normal: Vec3,
    /// Barycentric coordinates of the hit in the triangle.
    pub barycentric: Vec3,
    /// Distance along the ray.
    pub distance: f32,
}

/// One mesh taking part in a pick.
#[derive(Debug, Clone, Copy)]
pub struct PickTarget<'a> {
    /// Mesh id reported in [`Pick::mesh_id`].
    pub id: MeshId,
    /// The mesh (local coordinates).
    pub mesh: &'a TriMesh,
    /// BVH built from `mesh`.
    pub bvh: &'a Bvh,
    /// Local to world transform.
    pub xform: Mat4,
}

/// Nearest hit of `ray` (unit direction) among `targets`.
pub fn pick<'a>(ray: &Ray, targets: impl IntoIterator<Item = PickTarget<'a>>) -> Option<Pick> {
    let mut best: Option<Pick> = None;
    for t in targets {
        if t.xform.determinant().abs() < f32::EPSILON {
            continue;
        }
        let inv = t.xform.inverse();
        // the local direction is not normalised, so `t` stays the world distance
        let origin = inv.transform_point3(ray.origin);
        let dir = inv.transform_vector3(ray.dir);
        let limit = best.map_or(f32::INFINITY, |b| b.distance);
        let Some(hit) = t.bvh.ray_cast(origin, dir, 0.0, limit) else {
            continue;
        };
        let normal_mat = Mat3::from_mat4(t.xform).inverse().transpose();
        let normal = (normal_mat * t.mesh.face_normal(hit.tri as usize)).normalize_or_zero();
        best = Some(Pick {
            mesh_id: t.id,
            tri: hit.tri,
            point: ray.at(hit.t),
            normal,
            barycentric: hit.barycentric,
            distance: hit.t,
        });
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Camera, StdView};
    use glam::Vec2;

    /// Axis-aligned cube `[-1, 1]³` with outward normals.
    fn cube() -> TriMesh {
        let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let positions = vec![
            p(-1., -1., -1.),
            p(1., -1., -1.),
            p(1., 1., -1.),
            p(-1., 1., -1.),
            p(-1., -1., 1.),
            p(1., -1., 1.),
            p(1., 1., 1.),
            p(-1., 1., 1.),
        ];
        let tris = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [2, 3, 7],
            [2, 7, 6],
            [1, 2, 6],
            [1, 6, 5],
            [0, 4, 7],
            [0, 7, 3],
        ];
        TriMesh::new(positions, tris)
    }

    #[test]
    fn picks_nearest_of_two_transformed_meshes() {
        let m = cube();
        let bvh = Bvh::build(&m);
        let near = Mat4::from_translation(Vec3::new(0.0, -10.0, 0.0));
        let far =
            Mat4::from_translation(Vec3::new(0.0, 10.0, 0.0)) * Mat4::from_scale(Vec3::splat(3.0));
        let ray = Ray {
            origin: Vec3::new(0.2, -50.0, 0.1),
            dir: Vec3::Y,
        };
        let targets = [
            PickTarget {
                id: 2,
                mesh: &m,
                bvh: &bvh,
                xform: far,
            },
            PickTarget {
                id: 1,
                mesh: &m,
                bvh: &bvh,
                xform: near,
            },
        ];
        let p = pick(&ray, targets).unwrap();
        assert_eq!(p.mesh_id, 1);
        assert!((p.point - Vec3::new(0.2, -11.0, 0.1)).length() < 1e-4);
        assert!((p.distance - 39.0).abs() < 1e-4);
        assert!((p.normal - Vec3::NEG_Y).length() < 1e-5);
        // only the far, scaled cube: its −Y face is at y = 7
        let p = pick(&ray, [targets[0]]).unwrap();
        assert_eq!(p.mesh_id, 2);
        assert!((p.point.y - 7.0).abs() < 1e-4);
        assert!((p.distance - 57.0).abs() < 1e-4);
    }

    #[test]
    fn rotated_mesh_normal_is_in_world() {
        let m = cube();
        let bvh = Bvh::build(&m);
        let x = Mat4::from_rotation_z(std::f32::consts::FRAC_PI_4);
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 10.0),
            dir: Vec3::NEG_Z,
        };
        let p = pick(
            &ray,
            [PickTarget {
                id: 7,
                mesh: &m,
                bvh: &bvh,
                xform: x,
            }],
        )
        .unwrap();
        assert!((p.point.z - 1.0).abs() < 1e-5);
        assert!((p.normal - Vec3::Z).length() < 1e-5);
        assert!(p.barycentric.min_element() >= 0.0);
    }

    #[test]
    fn camera_ray_picks_centre_of_front_view() {
        let m = cube();
        let bvh = Bvh::build(&m);
        let mut cam = Camera::default();
        cam.set_view(StdView::Front);
        cam.fit_box(Vec3::splat(-1.0), Vec3::splat(1.0), 1.0, 1.2);
        let ray = cam.ray(Vec2::ZERO, 1.0, None);
        let p = pick(
            &ray,
            [PickTarget {
                id: 0,
                mesh: &m,
                bvh: &bvh,
                xform: Mat4::IDENTITY,
            }],
        )
        .unwrap();
        // the front face of the cube is the one towards −Y (incisors side)
        assert!((p.point - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-3);
        assert!(
            pick(
                &cam.ray(Vec2::new(0.95, 0.95), 1.0, None),
                [PickTarget {
                    id: 0,
                    mesh: &m,
                    bvh: &bvh,
                    xform: Mat4::IDENTITY
                }]
            )
            .is_none()
        );
    }

    #[test]
    fn singular_transform_is_skipped() {
        let m = cube();
        let bvh = Bvh::build(&m);
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            dir: Vec3::NEG_Z,
        };
        let t = PickTarget {
            id: 0,
            mesh: &m,
            bvh: &bvh,
            xform: Mat4::from_scale(Vec3::ZERO),
        };
        assert!(pick(&ray, [t]).is_none());
    }
}
