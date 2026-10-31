//! Colourings of dental meshes: tooth labels with a [`LabelPalette`] (the
//! same palette as `dc-raster`, so renders match the viewport) and plain
//! per-vertex colours, plus their CPU resolution to one colour per vertex.

use std::sync::Arc;

use dc_mesh::TriMesh;
use dc_scene::{Color, LabelPalette, MeshColoring};

/// Colouring of `scan` by the per-vertex tooth labels of `labelled` (a mesh
/// whose `vertex_labels` hold the FDI numbers, `0` for the gingiva, e.g. the
/// Aligners module's labels blob). `None` when `labelled` has no labels or
/// not one per vertex of `scan`.
pub fn label_coloring(
    scan: &TriMesh,
    labelled: &TriMesh,
    palette: &LabelPalette,
) -> Option<MeshColoring> {
    let labels = labelled.vertex_labels.as_ref()?;
    (labels.len() == scan.vertex_count()).then(|| MeshColoring::Labels {
        labels: Arc::new(labels.clone()),
        palette: palette.clone(),
    })
}

/// Colour of every vertex of `labels` with `palette`.
pub fn label_colors(labels: &[u16], palette: &LabelPalette) -> Vec<Color> {
    labels.iter().map(|&l| palette.color(l)).collect()
}

/// The colour each vertex of `mesh` gets with `coloring`, as the viewport
/// draws it (per-vertex data of the wrong length falls back to the uniform
/// default tooth colour, as the GPU path does).
pub fn resolve_colors(mesh: &TriMesh, coloring: &MeshColoring) -> Vec<Color> {
    let nv = mesh.vertex_count();
    let fallback = || vec![Color::TOOTH; nv];
    match coloring {
        MeshColoring::Uniform(c) => vec![*c; nv],
        MeshColoring::PerVertex(c) if c.len() == nv => c.to_vec(),
        MeshColoring::VertexLabels(p) => match &mesh.vertex_labels {
            Some(l) if l.len() == nv => label_colors(l, p),
            _ => fallback(),
        },
        MeshColoring::Labels { labels, palette } if labels.len() == nv => {
            label_colors(labels, palette)
        }
        _ => fallback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn tri() -> TriMesh {
        TriMesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]])
    }

    #[test]
    fn labels_colour_the_scan_vertex_by_vertex() {
        let scan = tri();
        let mut labelled = tri();
        labelled.vertex_labels = Some(vec![11, 0, 36]);
        let p = LabelPalette::default();
        let c = label_coloring(&scan, &labelled, &p).unwrap();
        let colors = resolve_colors(&scan, &c);
        assert_eq!(colors, vec![p.color(11), p.unlabeled, p.color(36)]);
        assert_ne!(colors[0], colors[2]);
        // the gingiva has the palette's unlabeled colour
        assert_eq!(colors[1], Color::GINGIVA);
    }

    #[test]
    fn mismatched_or_missing_labels_give_no_colouring() {
        let scan = tri();
        let mut labelled = tri();
        assert!(label_coloring(&scan, &labelled, &LabelPalette::default()).is_none());
        labelled.vertex_labels = Some(vec![11, 11]);
        assert!(label_coloring(&scan, &labelled, &LabelPalette::default()).is_none());
        // a wrong-length explicit colouring falls back to the tooth colour
        let bad = MeshColoring::PerVertex(Arc::new(vec![Color::WHITE]));
        assert_eq!(resolve_colors(&scan, &bad), vec![Color::TOOTH; 3]);
    }

    #[test]
    fn every_colouring_resolves_to_one_colour_per_vertex() {
        let mut scan = tri();
        scan.vertex_labels = Some(vec![21, 21, 0]);
        let p = LabelPalette::default();
        assert_eq!(
            resolve_colors(&scan, &MeshColoring::VertexLabels(p.clone())),
            vec![p.color(21), p.color(21), p.unlabeled]
        );
        let per = vec![Color::WHITE, Color::BLACK, Color::TOOTH];
        assert_eq!(
            resolve_colors(&scan, &MeshColoring::PerVertex(Arc::new(per.clone()))),
            per
        );
        assert_eq!(
            resolve_colors(&scan, &MeshColoring::Uniform(Color::WHITE)),
            vec![Color::WHITE; 3]
        );
    }
}
