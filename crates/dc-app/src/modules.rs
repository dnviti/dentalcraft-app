//! Modules shown on Home and in the New project chooser: the compiled-in
//! modules of the [`ModuleRegistry`] plus static "coming soon" descriptors.

use dc_module_api::{Module, ModuleRegistry};
use dc_ui::icons;

/// A module card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    /// Module id (`aligners`).
    pub id: &'static str,
    /// Shade-tab code (`AL`).
    pub code: &'static str,
    /// Phosphor icon.
    pub icon: &'static str,
    /// i18n key of the name (any registered bundle).
    pub name_key: &'static str,
    /// i18n key (dc-app) of the one-line description.
    pub desc_key: &'static str,
    /// Installed version, `None` for modules that are not available yet.
    pub version: Option<&'static str>,
    /// i18n key (dc-app) of the expected release, for coming-soon modules.
    pub eta_key: Option<&'static str>,
}

impl ModuleInfo {
    /// Installed (compiled in and enabled).
    pub fn installed(&self) -> bool {
        self.version.is_some()
    }
}

/// Modules that are announced but not available yet.
pub const COMING_SOON: [ModuleInfo; 4] = [
    ModuleInfo {
        id: "model-builder",
        code: "MB",
        icon: icons::CUBE_FOCUS,
        name_key: "module-model-builder",
        desc_key: "module-model-builder-desc",
        version: None,
        eta_key: Some("module-eta-spring-2027"),
    },
    ModuleInfo {
        id: "splint",
        code: "SP",
        icon: icons::SHIELD,
        name_key: "module-splint",
        desc_key: "module-splint-desc",
        version: None,
        eta_key: Some("module-eta-2027"),
    },
    ModuleInfo {
        id: "crown-bridge",
        code: "CB",
        icon: icons::CROWN_SIMPLE,
        name_key: "module-crown-bridge",
        desc_key: "module-crown-bridge-desc",
        version: None,
        eta_key: Some("module-eta-2027"),
    },
    ModuleInfo {
        id: "implant",
        code: "IM",
        icon: icons::SCREWDRIVER,
        name_key: "module-implant",
        desc_key: "module-implant-desc",
        version: None,
        eta_key: Some("module-eta-2028"),
    },
];

/// The registry of compiled-in modules.
pub fn registry() -> ModuleRegistry {
    let mut reg = ModuleRegistry::new();
    if let Err(e) = dca_module::register(&mut reg) {
        tracing::error!("cannot register the aligners module: {e}");
    }
    reg
}

/// A fresh instance of module `id` for a workspace.
pub fn instantiate(id: &str) -> Option<Box<dyn Module>> {
    match id {
        "aligners" => Some(Box::new(dca_module::AlignersModule::new())),
        _ => None,
    }
}

fn installed_info(m: &dyn Module) -> ModuleInfo {
    let (code, icon, desc_key) = match m.id() {
        "aligners" => ("AL", icons::TOOTH, "module-aligners-desc"),
        _ => ("??", icons::CUBE, "module-generic-desc"),
    };
    ModuleInfo {
        id: m.id(),
        code,
        icon,
        name_key: m.name_key(),
        desc_key,
        version: Some(m.version()),
        eta_key: None,
    }
}

/// Installed modules first (registry order), then the coming-soon ones.
pub fn cards(reg: &ModuleRegistry) -> Vec<ModuleInfo> {
    reg.enabled()
        .map(installed_info)
        .chain(COMING_SOON.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligners_is_installed_and_instantiable() {
        let reg = registry();
        let cards = cards(&reg);
        assert_eq!(cards.len(), 5);
        assert_eq!(cards[0].id, "aligners");
        assert!(cards[0].installed());
        assert!(cards[1..].iter().all(|c| !c.installed()));
        assert_eq!(instantiate("aligners").unwrap().id(), "aligners");
        assert!(instantiate("implant").is_none());
    }
}
