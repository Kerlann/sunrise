//! The material family the book's how-to page includes. A mod's real family is data; this one keeps
//! the page compiled.

use mod_api::{Form, ItemKey, Registrar};

/// `(id, tag)` for `form` of `material`: the pair a declaration and its tag file share.
fn pair(form: Form, material: &str) -> (String, String) {
    (
        form.id(material)
            .expect("the page's materials are legal resource paths"),
        form.tag(material)
            .expect("the page's materials are legal resource paths"),
    )
}

// ANCHOR: family
/// Every id and tag the steel family suggests, in the order a GT mod registers them.
pub fn steel_family() -> Vec<(String, String)> {
    Form::ALL
        .iter()
        .copied()
        .map(|form| pair(form, "steel"))
        .collect()
}
// ANCHOR_END: family

// ANCHOR: declare
/// Request the steel items; the ore and storage block go through `block_key` the same way.
pub fn declare_steel_items(registrar: &mut dyn Registrar, namespace: &str) -> Vec<ItemKey> {
    Form::ALL
        .iter()
        .copied()
        .filter(|form| !form.is_block())
        .map(|form| {
            let id = form.id("steel").expect("steel is a legal resource path");
            registrar.item_key(&format!("{namespace}:{id}"))
        })
        .collect()
}
// ANCHOR_END: declare

// ANCHOR: explicit
/// A gem whose id is not the suggestion: vanilla's gem is `amethyst_shard`, not `amethyst`, but it
/// still joins `c:gems/amethyst`, so the convention tag is independent of the id the mod picks. This
/// requests the key and builds the tag name; registering the item and its tag membership are separate.
pub fn amethyst_gem(registrar: &mut dyn Registrar, namespace: &str) -> (ItemKey, String) {
    let id = registrar.item_key(&format!("{namespace}:amethyst_shard"));
    let tag = Form::Gem
        .tag("amethyst")
        .expect("amethyst is a legal resource path");
    (id, tag)
}
// ANCHOR_END: explicit
