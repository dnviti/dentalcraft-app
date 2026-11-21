//! The words of the shell: what a dentist or a technician reads carries no
//! implementation name, no citation and no jargon of the pipeline.

use std::path::PathBuf;

/// `true`: compared as a whole word. All in lower case.
const BANNED: &[(&str, bool)] = &[
    ("icp", true),
    ("6-dof", false),
    ("blake3", false),
    ("manifold", false),
    ("watertight", false),
    ("bvh", true),
    ("voxel", false),
    ("signed distance", false),
    ("harmonic", false),
    ("random walk", false),
    ("walker", false),
    ("fairing", false),
    ("polynomial", false),
    ("rigid transform", false),
    ("world frame", false),
    ("mesh", true),
    ("meshes", true),
    ("kobbelt", false),
    ("alharbi", false),
    ("b612", false),
    ("blender", false),
    ("seed", true),
    ("seeds", true),
    ("seme", true),
    ("semi", true),
    ("label", true),
    ("labels", true),
    ("etichetta", true),
    ("etichette", true),
    ("segmentation", false),
    ("segmentazione", false),
    ("reconstruction", false),
    ("ricostruzione", false),
    ("staging", false),
    ("stadiazione", false),
    ("running command", false),
    ("comando in corso", false),
    ("command failed", false),
    ("comando non riuscito", false),
    ("case info", false),
    ("dati del caso", false),
    // the print popup: the words of the slicer and of the printer's protocol
    ("slice", true),
    ("slicing", true),
    ("goo", true),
    ("sdcp", true),
    ("rle", true),
];

/// Keys that keep a word with another meaning: not the colours of the teeth
/// but the labels printed on a model, and the badge of a scan.
const ALLOWED: &[&str] = &["module-model-builder-desc", "np-drop-body"];

fn has(text: &str, word: &str, whole: bool) -> bool {
    if whole {
        text.split(|c: char| !c.is_alphanumeric() && c != '-')
            .any(|w| w == word)
    } else {
        text.contains(word)
    }
}

#[test]
fn no_banned_word_in_the_text_of_the_shell() {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let mut lines = 0;
    for krate in [
        "dc-app",
        "dc-ui",
        "dc-workspace",
        "dc-scan-ui",
        "dc-print-ui",
    ] {
        for lang in ["en", "it"] {
            let path = crates
                .join(krate)
                .join("i18n")
                .join(lang)
                .join(format!("{krate}.ftl"));
            for line in std::fs::read_to_string(path).unwrap().lines() {
                let Some((key, text)) = line.split_once(" = ") else {
                    continue;
                };
                if key.starts_with('#') || ALLOWED.contains(&key) {
                    continue;
                }
                lines += 1;
                let text = text.to_lowercase();
                for (word, whole) in BANNED {
                    assert!(!has(&text, word, *whole), "{krate} {lang} {key}: {text}");
                }
            }
        }
    }
    assert!(lines > 500, "only {lines} lines read");
}

#[test]
fn the_empty_timeline_names_the_menu() {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("i18n");
    let read = |lang: &str| std::fs::read_to_string(crates.join(lang).join("dc-workspace.ftl"));
    assert!(read("en").unwrap().contains("in Plan, Aligners."));
    assert!(read("it").unwrap().contains("in Piano, Allineatori."));
}
