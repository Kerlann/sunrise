# `ENTITY_ITEMS` bound-armour extraction checks creative game mode

The built-in player handler for `ENTITY_ITEMS` (`ArmorSlot.mayPickup`) now decides whether a stack with the `PREVENT_ARMOR_CHANGE` effect may be extracted from an armour slot by creative game mode (`Player.isCreative`), not the `instabuild` / infinite-materials ability (#6972).

Extracting bound armour returns `Ok(0)` outside creative mode even when the player has infinite materials enabled; `ItemPlace::InfiniteMaterials` still follows the infinite-materials ability.
