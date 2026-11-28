//! Registration and lookup plumbing for mod-declared load conditions
//! (`neoforge:conditions` and `fabric:load_conditions` custom handlers).
use super::*;
use crate::content::load_conditions::{LoadCondition, RegisteredLoadCondition};

/// Object-safe low-level registrar trait for custom load conditions.
/// Mods call [`Registrar::register_load_condition`] instead.
// hidden: typed helpers live on `dyn Registrar`; this object-safe layer is internal plumbing.
#[doc(hidden)]
pub trait LoadConditionsRegistrar {
    /// Register one boxed custom load condition handler under `id`.
    fn register_load_condition_erased(&mut self, id: &str, condition: Box<dyn LoadCondition>);
}

impl dyn Registrar + '_ {
    /// Register a custom load condition handler under canonical namespaced `id`
    /// during [`Hooks::register`](crate::Hooks::register).
    ///
    /// IDs may use any valid namespace, except built-in `neoforge:*` and `fabric:*`
    /// condition IDs. Invalid IDs, built-in IDs, and duplicate IDs across mods are
    /// refused at startup naming the mod(s).
    pub fn register_load_condition<C: LoadCondition + 'static>(&mut self, id: &str, condition: C) {
        self.register_load_condition_erased(id, Box::new(condition));
    }
}

/// Built-in `neoforge:*` condition paths (`sunrise_world::conditions::BuiltinCondition`).
pub const BUILTIN_NEOFORGE_CONDITIONS: &[&str] = &[
    "mod_loaded",
    "always",
    "never",
    "feature_flags_enabled",
    "registered",
    "tag_empty",
    "not",
    "and",
    "or",
];

/// Built-in `fabric:*` condition paths (`DefaultResourceConditionTypes`).
pub const BUILTIN_FABRIC_CONDITIONS: &[&str] = &[
    "true",
    "false",
    "not",
    "and",
    "or",
    "all_mods_loaded",
    "any_mods_loaded",
    "tags_populated",
    "features_enabled",
    "registry_contains",
];

/// Whether `id` is a built-in NeoForge or Fabric load condition (single source of truth
/// shared by startup registration validation and the world condition evaluator).
pub fn is_builtin_condition(id: &crate::Identifier) -> bool {
    (id.namespace() == "neoforge" && BUILTIN_NEOFORGE_CONDITIONS.contains(&id.path()))
        || (id.namespace() == "fabric" && BUILTIN_FABRIC_CONDITIONS.contains(&id.path()))
}

/// All startup-frozen custom load conditions in the current installed epoch (`TEST_PARTS`
/// first, then the process-wide `INSTALLED`).
pub fn installed_load_conditions() -> &'static [RegisteredLoadCondition] {
    installed().map_or(&[], |installed| &installed.load_conditions)
}
