//! The tag `#minecraft:redirectable_projectile` is read as bound
//! (`HeldItemsGameExt1::projectile_redirectable`, `projectile_is_pickable`), so a data pack's change
//! to it applies.
/// `AbstractHurtingProjectile.getPickRadius`: the crosshair finds it a block
/// round its box.
pub const PICK_RADIUS: f64 = 1.0;
