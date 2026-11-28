//! The material family the book's how-to page includes. A mod's real family is data; this one keeps
//! the page compiled.

use mod_api::{Form, ItemKey, Registrar};

/// `(id, tag)` for `form` of `material`: the pair a declaration and its tag file share.
fn pair(form: Form, material: &str) -> (String, String) {
    (
        form.id(material)
            .expect("the page's materials are snake_case"),
        form.tag(material)
            .expect("the page's materials are snake_case"),
    )
}

// ANCHOR: family
/// Every id and tag the steel family declares, in the order a GT mod registers them.
pub fn steel_family() -> Vec<(String, String)> {
    Form::ALL
        .iter()
        .copied()
        .map(|form| pair(form, "steel"))
        .collect()
}
// ANCHOR_END: family

// ANCHOR: declare
/// Request the steel items; the blocks go through `block_key` the same way.
pub fn declare_steel_items(registrar: &mut dyn Registrar, namespace: &str) -> Vec<ItemKey> {
    Form::ALL
        .iter()
        .copied()
        .filter(|form| !form.is_block())
        .map(|form| {
            let id = form.id("steel").expect("steel is a snake_case material");
            registrar.item_key(&format!("{namespace}:{id}"))
        })
        .collect()
}
// ANCHOR_END: declare

// ANCHOR: legacy
/// Read a 1.7.10 ore-dictionary name as the declaration a port writes.
pub fn ported_declaration(ore_name: &str) -> Option<(String, String)> {
    let ore = mod_api::parse_ore_name(ore_name)?;
    Some((ore.id(), ore.tag()))
}
// ANCHOR_END: legacy
