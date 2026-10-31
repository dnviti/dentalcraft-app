//! Compact bounding volume hierarchy over the triangles of a mesh, used for
//! CPU picking.
//!
//! Built with median splits along the longest centroid axis; leaves hold up
//! to [`LEAF_SIZE`] triangles. Ray tests are two-sided (scans are open
//! surfaces and may be seen from the back).

use dc_mesh::TriMesh;
use glam::Vec3;

/// Maximum number of triangles in a leaf.
pub const LEAF_SIZE: usize = 4;

#[derive(Debug, Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: first index into `order`; inner: index of the right child (the
    /// left child immediately follows its parent).
    start_or_right: u32,
    /// Number of triangles for a leaf, `0` for an inner node.
    count: u32,
}

/// Nearest ray hit on a mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Triangle index.
    pub tri: u32,
    /// Ray parameter (distance when the direction has unit length).
    pub t: f32,
    /// Barycentric coordinates `(w0, w1, w2)` of the hit point.
    pub barycentric: Vec3,
}

/// BVH over the triangles of one mesh (mesh-local coordinates).
#[derive(Debug, Clone, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
    tris: Vec<[Vec3; 3]>,
}

impl Bvh {
    /// Builds the hierarchy. Triangles with out-of-range indices are skipped.
    pub fn build(mesh: &TriMesh) -> Self {
        let nv = mesh.positions.len();
        let tris: Vec<[Vec3; 3]> = mesh
            .tris
            .iter()
            .map(|t| {
                if t.iter().all(|&i| (i as usize) < nv) {
                    t.map(|i| mesh.positions[i as usize])
                } else {
                    [Vec3::NAN; 3]
                }
            })
            .collect();
        let mut order: Vec<u32> = (0..tris.len() as u32)
            .filter(|&i| tris[i as usize][0].is_finite())
            .collect();
        let centroids: Vec<Vec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
        let mut nodes = Vec::with_capacity(2 * order.len() / LEAF_SIZE + 1);
        if !order.is_empty() {
            build_rec(&tris, &centroids, &mut order, 0, &mut nodes);
        }
        Self { nodes, order, tris }
    }

    /// Number of nodes (for tests and diagnostics).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Nearest hit of the ray `origin + t * dir` with `t` in `(t_min, t_max)`.
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3, t_min: f32, t_max: f32) -> Option<RayHit> {
        if self.nodes.is_empty() {
            return None;
        }
        // a huge finite inverse instead of infinity avoids `0 * inf = NaN`
        // in the slab test for axis-parallel rays
        let inv = Vec3::select(
            dir.abs().cmplt(Vec3::splat(1e-30)),
            Vec3::splat(1e30),
            dir.recip(),
        );
        let mut best: Option<RayHit> = None;
        let mut limit = t_max;
        let mut stack = Vec::with_capacity(64);
        stack.push(0usize);
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni];
            if slab(origin, inv, n.min, n.max, t_min, limit).is_none() {
                continue;
            }
            if n.count > 0 {
                let s = n.start_or_right as usize;
                for &ti in &self.order[s..s + n.count as usize] {
                    if let Some((t, b)) = ray_triangle(origin, dir, &self.tris[ti as usize])
                        && t > t_min
                        && t < limit
                    {
                        limit = t;
                        best = Some(RayHit {
                            tri: ti,
                            t,
                            barycentric: b,
                        });
                    }
                }
            } else {
                let (l, r) = (ni + 1, n.start_or_right as usize);
                // visit the nearer child first
                let dl = slab(
                    origin,
                    inv,
                    self.nodes[l].min,
                    self.nodes[l].max,
                    t_min,
                    limit,
                );
                let dr = slab(
                    origin,
                    inv,
                    self.nodes[r].min,
                    self.nodes[r].max,
                    t_min,
                    limit,
                );
                match (dl, dr) {
                    (Some(a), Some(b)) if a <= b => {
                        stack.push(r);
                        stack.push(l);
                    }
                    (Some(_), Some(_)) => {
                        stack.push(l);
                        stack.push(r);
                    }
                    (Some(_), None) => stack.push(l),
                    (None, Some(_)) => stack.push(r),
                    (None, None) => {}
                }
            }
        }
        best
    }
}

fn build_rec(
    tris: &[[Vec3; 3]],
    centroids: &[Vec3],
    order: &mut [u32],
    start: usize,
    nodes: &mut Vec<Node>,
) -> usize {
    let (mut min, mut max) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    let (mut cmin, mut cmax) = (min, max);
    for &i in order.iter() {
        for p in tris[i as usize] {
            min = min.min(p);
            max = max.max(p);
        }
        let c = centroids[i as usize];
        cmin = cmin.min(c);
        cmax = cmax.max(c);
    }
    let me = nodes.len();
    nodes.push(Node {
        min,
        max,
        start_or_right: start as u32,
        count: order.len() as u32,
    });
    let extent = cmax - cmin;
    if order.len() <= LEAF_SIZE || extent.max_element() <= 0.0 {
        return me;
    }
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    let mid = order.len() / 2;
    order.select_nth_unstable_by(mid, |a, b| {
        centroids[*a as usize][axis].total_cmp(&centroids[*b as usize][axis])
    });
    let (left, right) = order.split_at_mut(mid);
    build_rec(tris, centroids, left, start, nodes);
    let r = build_rec(tris, centroids, right, start + mid, nodes);
    nodes[me].start_or_right = r as u32;
    nodes[me].count = 0;
    me
}

/// Entry parameter of the ray into the box, if it hits within `(t0, t1)`.
fn slab(origin: Vec3, inv: Vec3, min: Vec3, max: Vec3, t0: f32, t1: f32) -> Option<f32> {
    let a = (min - origin) * inv;
    let b = (max - origin) * inv;
    let tn = a.min(b).max_element().max(t0);
    let tf = a.max(b).min_element().min(t1);
    (tn <= tf).then_some(tn)
}

/// Two-sided Möller–Trumbore ray/triangle intersection.
pub(crate) fn ray_triangle(origin: Vec3, dir: Vec3, tri: &[Vec3; 3]) -> Option<(f32, Vec3)> {
    let e1 = tri[1] - tri[0];
    let e2 = tri[2] - tri[0];
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < f32::EPSILON * e1.length_squared().max(e2.length_squared()) * 1e-3 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - tri[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    Some((t, Vec3::new(1.0 - u - v, u, v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wavy height field of `n * n * 2` triangles over `[0, n]²`.
    fn terrain(n: u32) -> TriMesh {
        let mut pos = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (i as f32, j as f32);
                pos.push(Vec3::new(x, y, (x * 0.7).sin() * (y * 0.4).cos() * 2.0));
            }
        }
        let mut tris = Vec::new();
        let w = n + 1;
        for j in 0..n {
            for i in 0..n {
                let a = j * w + i;
                tris.push([a, a + 1, a + w + 1]);
                tris.push([a, a + w + 1, a + w]);
            }
        }
        TriMesh::new(pos, tris)
    }

    fn brute(mesh: &TriMesh, o: Vec3, d: Vec3) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        for f in 0..mesh.tris.len() {
            if let Some((t, _)) = ray_triangle(o, d, &mesh.triangle(f))
                && t > 0.0
                && best.is_none_or(|(_, bt)| t < bt)
            {
                best = Some((f as u32, t));
            }
        }
        best
    }

    /// Tiny deterministic generator (no extra dependency).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
        }
    }

    #[test]
    fn matches_brute_force() {
        let mesh = terrain(24);
        let bvh = Bvh::build(&mesh);
        assert!(bvh.node_count() > 1);
        let mut rng = Lcg(42);
        let mut hits = 0;
        for _ in 0..500 {
            let o = Vec3::new(
                rng.next() * 24.0,
                rng.next() * 24.0,
                10.0 + rng.next() * 5.0,
            );
            let d = Vec3::new(rng.next() - 0.5, rng.next() - 0.5, -1.0).normalize();
            let a = bvh.ray_cast(o, d, 0.0, f32::INFINITY);
            let b = brute(&mesh, o, d);
            match (a, b) {
                (Some(a), Some((_, bt))) => {
                    hits += 1;
                    assert!((a.t - bt).abs() < 1e-4, "{} vs {bt}", a.t);
                }
                (None, None) => {}
                (a, b) => panic!("bvh {a:?} brute {b:?}"),
            }
        }
        assert!(hits > 300);
    }

    #[test]
    fn hits_from_both_sides_and_respects_range() {
        let mesh = TriMesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]);
        let bvh = Bvh::build(&mesh);
        let above = bvh.ray_cast(Vec3::new(0.2, 0.2, 1.0), Vec3::NEG_Z, 0.0, 10.0);
        let below = bvh.ray_cast(Vec3::new(0.2, 0.2, -1.0), Vec3::Z, 0.0, 10.0);
        assert_eq!(above.unwrap().tri, 0);
        assert!((below.unwrap().t - 1.0).abs() < 1e-6);
        let b = above.unwrap().barycentric;
        assert!((b - Vec3::new(0.6, 0.2, 0.2)).length() < 1e-5);
        assert!(
            bvh.ray_cast(Vec3::new(0.2, 0.2, 1.0), Vec3::NEG_Z, 0.0, 0.5)
                .is_none()
        );
        assert!(
            bvh.ray_cast(Vec3::new(2.0, 2.0, 1.0), Vec3::NEG_Z, 0.0, 10.0)
                .is_none()
        );
    }

    #[test]
    fn empty_and_invalid_meshes() {
        let bvh = Bvh::build(&TriMesh::default());
        assert!(bvh.ray_cast(Vec3::ZERO, Vec3::Z, 0.0, 1.0).is_none());
        let bad = TriMesh::new(vec![Vec3::ZERO], vec![[0, 1, 2]]);
        let bvh = Bvh::build(&bad);
        assert!(bvh.ray_cast(Vec3::ZERO, Vec3::Z, -1.0, 1.0).is_none());
    }

    #[test]
    fn axis_aligned_rays_hit() {
        let mesh = terrain(8);
        let bvh = Bvh::build(&mesh);
        // straight down: direction has zero x and y components (infinite inverse)
        let hit = bvh
            .ray_cast(Vec3::new(3.5, 4.25, 50.0), Vec3::NEG_Z, 0.0, f32::INFINITY)
            .unwrap();
        let b = brute(&mesh, Vec3::new(3.5, 4.25, 50.0), Vec3::NEG_Z).unwrap();
        assert!((hit.t - b.1).abs() < 1e-4);
    }
}
