//! "Import scans": reading STL / OBJ / PLY files in the background and the
//! undoable command that adds them to the case.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dc_case::{CaseState, Pose, Scan, ScanRole};
use dc_command::{Command, CommandError, JobCtx};
use dc_mesh::{TriMesh, Units};

/// Last segment of the command ids the workspace implements itself as
/// "Import scans" (e.g. `aligners.case.import-scans`), whatever the module.
pub const IMPORT_SCANS_SUFFIX: &str = "import-scans";

/// Command id of the import, for the undo stack.
pub const IMPORT_COMMAND_ID: &str = "workspace.import-scans";

/// File extensions offered by the file dialog.
pub const EXTENSIONS: [&str; 3] = ["stl", "obj", "ply"];

/// `true` when `command_id` is an "Import scans" entry.
pub fn is_import_command(command_id: &str) -> bool {
    command_id
        .rsplit('.')
        .next()
        .is_some_and(|last| last == IMPORT_SCANS_SUFFIX)
}

/// A scan read from disk, converted to millimetres.
#[derive(Debug, Clone)]
pub struct ImportedScan {
    /// Arch or bite.
    pub role: ScanRole,
    /// File name without directory.
    pub source_name: String,
    /// Detected unit of the file.
    pub source_units: Units,
    /// The mesh in millimetres.
    pub mesh: Arc<TriMesh>,
}

/// Role suggested by the file name (`upper`, `maxil…`, `superior`,
/// `lower`, `mandib…`, `inferior`, `bite`, `morso`), if any.
pub fn role_from_name(name: &str) -> Option<ScanRole> {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["bite", "morso"]) {
        Some(ScanRole::Bite)
    } else if has(&["upper", "maxil", "superior"]) {
        Some(ScanRole::Upper)
    } else if has(&["lower", "mandib", "inferior"]) {
        Some(ScanRole::Lower)
    } else {
        None
    }
}

/// Assigns a role to every file: the one its name suggests, otherwise the
/// first of upper, lower, bite not taken yet (by the case or by an earlier
/// file of the batch); upper when all are taken.
pub fn assign_roles(names: &[String], existing: &[ScanRole]) -> Vec<ScanRole> {
    let mut taken: Vec<ScanRole> = existing.to_vec();
    let hinted: Vec<Option<ScanRole>> = names.iter().map(|n| role_from_name(n)).collect();
    // hinted roles are reserved first, so an unhinted file does not grab them
    taken.extend(hinted.iter().flatten().copied());
    hinted
        .into_iter()
        .map(|h| {
            h.unwrap_or_else(|| {
                let free = [ScanRole::Upper, ScanRole::Lower, ScanRole::Bite]
                    .into_iter()
                    .find(|r| !taken.contains(r))
                    .unwrap_or(ScanRole::Upper);
                taken.push(free);
                free
            })
        })
        .collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Reads `paths` (job body): each file is read, its unit detected and
/// converted to millimetres. Fails on the first unreadable file.
pub fn read_scans(
    paths: &[PathBuf],
    existing: &[ScanRole],
    ctx: &JobCtx,
) -> Result<Vec<ImportedScan>, String> {
    let names: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
    let roles = assign_roles(&names, existing);
    let mut out = Vec::with_capacity(paths.len());
    let n = paths.len().max(1) as f32;
    for (i, ((path, name), role)) in paths.iter().zip(names).zip(roles).enumerate() {
        ctx.check_cancelled().map_err(|e| e.to_string())?;
        ctx.set_progress(i as f32 / n, Some("ws-job-import-reading"));
        let mut mesh = dc_mesh::io::read(path).map_err(|e| format!("{name}: {e}"))?;
        let units = dc_mesh::io::normalize_units(&mut mesh).map_err(|e| format!("{name}: {e}"))?;
        out.push(ImportedScan {
            role,
            source_name: name,
            source_units: units,
            mesh: Arc::new(mesh),
        });
    }
    ctx.check_cancelled().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Adds scans to the case; a scan replaces an existing one with the same role.
#[derive(Debug, Clone)]
pub struct ImportScans {
    /// The scans to add.
    pub scans: Vec<ImportedScan>,
}

impl Command for ImportScans {
    fn id(&self) -> &'static str {
        IMPORT_COMMAND_ID
    }

    fn label_key(&self) -> &str {
        "ws-import-scans"
    }

    fn apply(&self, state: &CaseState, ctx: &JobCtx) -> Result<CaseState, CommandError> {
        if self.scans.is_empty() {
            return Err(CommandError::Invalid("no scans to import".into()));
        }
        let mut next = state.clone();
        for s in &self.scans {
            ctx.check_cancelled()?;
            let blob = next.blobs.insert_arc(s.mesh.clone());
            next.case.scans.retain(|old| old.role != s.role);
            next.case.scans.push(Scan {
                id: uuid::Uuid::new_v4(),
                role: s.role,
                source_name: s.source_name.clone(),
                source_units: s.source_units,
                import_xform: Pose::default(),
                mesh: blob,
            });
        }
        next.case.scans.sort_by_key(|s| s.role);
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_command_ids() {
        assert!(is_import_command("aligners.case.import-scans"));
        assert!(is_import_command("import-scans"));
        assert!(!is_import_command("aligners.case.import-scans-x"));
        assert!(!is_import_command("aligners.case.case-info"));
    }

    #[test]
    fn roles_from_names_and_free_slots() {
        assert_eq!(
            role_from_name("Patient_UpperJaw.stl"),
            Some(ScanRole::Upper)
        );
        assert_eq!(role_from_name("mandibular.ply"), Some(ScanRole::Lower));
        assert_eq!(role_from_name("bite_left.obj"), Some(ScanRole::Bite));
        assert_eq!(role_from_name("scan1.stl"), None);
        let names = vec![
            "a.stl".to_owned(),
            "lower.stl".to_owned(),
            "b.stl".to_owned(),
        ];
        assert_eq!(
            assign_roles(&names, &[]),
            vec![ScanRole::Upper, ScanRole::Lower, ScanRole::Bite]
        );
        assert_eq!(
            assign_roles(&["x.stl".to_owned()], &[ScanRole::Upper]),
            vec![ScanRole::Lower]
        );
    }

    #[test]
    fn command_adds_and_replaces_by_role() {
        let mesh = Arc::new(TriMesh::new(
            vec![glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Y],
            vec![[0, 1, 2]],
        ));
        let scan = |role| ImportedScan {
            role,
            source_name: "s.stl".into(),
            source_units: Units::Millimeters,
            mesh: mesh.clone(),
        };
        let cmd = ImportScans {
            scans: vec![scan(ScanRole::Lower), scan(ScanRole::Upper)],
        };
        let ctx = JobCtx::detached();
        let s1 = cmd.apply(&CaseState::default(), &ctx).unwrap();
        assert_eq!(s1.case.scans.len(), 2);
        assert_eq!(s1.case.scans[0].role, ScanRole::Upper);
        assert!(s1.blobs.contains(&s1.case.scans[0].mesh));
        let s2 = ImportScans {
            scans: vec![scan(ScanRole::Upper)],
        }
        .apply(&s1, &ctx)
        .unwrap();
        assert_eq!(s2.case.scans.len(), 2);
        assert!(ImportScans { scans: Vec::new() }.apply(&s2, &ctx).is_err());
    }
}
