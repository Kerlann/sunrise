//! NeoForge's built-in block data maps (`NeoForgeDataMaps`): `neoforge:flammable`,
//! `neoforge:oxidizables` and `neoforge:waxables`, merged from the world's
//! data packs and mods' files as a mod's own data maps are, and installed on
//! the world's behaviours for fire spread, weathering, scraping and waxing.
use crate::game::ServerGame;
use serde_json::Value;
use std::sync::Arc;
use sunrise_core::identifier::Identifier;
use sunrise_gameplay_common::game::native_data_maps_common::{MapRegistries, load_maps};
use sunrise_world::level::{BlockMapValue, BlockMaps};

/// The three maps' ids, all on `minecraft:block`.
const MAPS: [&str; 3] = [
    "neoforge:flammable",
    "neoforge:oxidizables",
    "neoforge:waxables",
];

/// Load the world's block maps. Resolve targets against `block_id` before merging each layer,
/// as NeoForge's `Waxable` and `Oxidizable` codecs do. The block id also orders inverse ties.
pub(in crate::game) fn load(
    overlay: &sunrise_world::loot::PackOverlay<'_>,
    registries: &dyn MapRegistries,
    block_id: impl Fn(&str) -> Option<u32>,
) -> Option<Arc<BlockMaps>> {
    let block_registry = Identifier::parse_namespaced("minecraft:block").unwrap();
    let types: Vec<_> = MAPS
        .into_iter()
        .map(|id| {
            Arc::new(mod_api::__private::RegisteredDataMap::from_raw(
                "neoforge",
                block_registry.clone(),
                Identifier::parse_namespaced(id).unwrap(),
                mod_api::RawDataMap::new("minecraft:block", id),
            ))
        })
        .collect();
    let mut maps = load_maps(
        &types,
        overlay,
        registries,
        |id, value| {
            let (field, transform): (&str, fn(String) -> BlockMapValue) = match id {
                "neoforge:flammable" => {
                    let read_odds = |field| {
                        value
                            .get(field)
                            .and_then(Value::as_i64)
                            .and_then(|odds| i32::try_from(odds).ok())
                            .ok_or_else(|| format!("expected an integer in {field}"))
                    };
                    return Ok(BlockMapValue::Flammable {
                        ignite_odds: read_odds("ignite_odds")?,
                        burn_odds: read_odds("burn_odds")?,
                    });
                }
                "neoforge:oxidizables" => ("next_oxidation_stage", BlockMapValue::Oxidizable),
                "neoforge:waxables" => ("waxed", BlockMapValue::Waxable),
                _ => unreachable!("only the built-in block map types are registered here"),
            };
            // NeoForge reads this flag for its generated block transformers, which Sunrise does
            // not install. An invalid flag still makes the value undecodable.
            if !matches!(
                value.get("generate_block_transform"),
                None | Some(Value::Bool(_))
            ) {
                return Err("expected a boolean in generate_block_transform".into());
            }
            let target = value
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("expected a block identifier in {field}"))?;
            let name = Identifier::parse(target)
                .map_err(|error| format!("{field}: {error}"))?
                .to_string();
            if block_id(&name).is_none() {
                return Err(format!("{field}: unknown block {name}"));
            }
            Ok(transform(name))
        },
        |_, _| Err("data map does not support partial removers".into()),
        |_, _, _, incoming| Ok(incoming.clone()),
        |_, _, _, _| Err("data map does not support partial removers".into()),
    );
    let blocks = maps.remove("minecraft:block")?;
    let block_maps = BlockMaps::from_values(blocks.into_values().flatten(), |name| {
        block_id(name).unwrap_or(u32::MAX)
    });
    (!block_maps.is_empty()).then(|| Arc::new(block_maps))
}

impl ServerGame<'_> {
    /// Install the open world's tables' block maps, replacing the last
    /// load's: none for a world without tables.
    pub(in crate::game::server_side) fn install_block_maps(&self) {
        let maps = self
            .server
            .world_tables
            .0
            .as_ref()
            .and_then(|tables| tables.block_maps.clone());
        self.common.res.behaviours().set_block_maps(maps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sunrise_gameplay_common::game::native_data_maps_common::WorldRegistries;

    #[test]
    fn pack_files_give_the_block_maps() {
        let file = |value: serde_json::Value| vec![serde_json::to_vec(&value).unwrap()];
        let files = vec![
            (
                "data_maps/neoforge:block/flammable".to_owned(),
                file(json!({"values": {"minecraft:stone": {"ignite_odds": 5, "burn_odds": 20}}})),
            ),
            (
                "data_maps/neoforge:block/waxables".to_owned(),
                file(json!({"values": {
                    "test:tin": {"waxed": "test:waxed_tin"},
                    "minecraft:stone": {
                        "waxed": "test:waxed_tin",
                        "neoforge:conditions": [{"type": "neoforge:never"}]
                    }
                }})),
            ),
        ];
        let overlay = sunrise_world::loot::PackOverlay {
            files: &files,
            ..sunrise_world::loot::PackOverlay::default()
        };
        let ids = ["minecraft:stone", "test:tin", "test:waxed_tin"].map(String::from);
        let items = sunrise_items::items::Items::empty();
        let registries = WorldRegistries::new(
            [("minecraft:block", ids.as_slice())],
            None,
            &items,
            std::path::Path::new(""),
            &overlay,
        );
        let block_id = |name: &str| ids.iter().position(|id| id == name).map(|i| i as u32);
        let block_maps = load(&overlay, &registries, block_id).unwrap();
        assert_eq!(block_maps.flammable("minecraft:stone"), Some((5, 20)));
        assert_eq!(block_maps.waxed("test:tin"), Some("test:waxed_tin"));
        assert_eq!(block_maps.waxed("minecraft:stone"), None);
        assert_eq!(block_maps.next_oxidation("test:tin"), None);
        let none = load(
            &sunrise_world::loot::PackOverlay::NONE,
            &registries,
            block_id,
        );
        assert!(none.is_none());
    }

    #[test]
    fn an_undecodable_block_value_skips_the_entire_layer_even_with_replace() {
        let layers = |values: &[serde_json::Value]| {
            values
                .iter()
                .map(|value| serde_json::to_vec(value).unwrap())
                .collect::<Vec<_>>()
        };
        let files = vec![
            (
                "data_maps/neoforge:block/flammable".to_owned(),
                layers(&[
                    json!({"values": {"minecraft:stone": {"ignite_odds": 5, "burn_odds": 20}}}),
                    json!({"values": {"minecraft:stone": null}}),
                    json!({
                        "replace": true,
                        "values": {"test:tin": {"ignite_odds": 1, "burn_odds": 2}},
                        "remove": {"minecraft:stone": 1}
                    }),
                ]),
            ),
            (
                "data_maps/neoforge:block/oxidizables".to_owned(),
                layers(&[
                    json!({"values": {"test:tin": {"next_oxidation_stage": "test:weathered_tin"}}}),
                    json!({"replace": true, "values": {"test:tin": {
                        "next_oxidation_stage": "test:waxed_tin",
                        "generate_block_transform": "true"
                    }}}),
                ]),
            ),
            (
                "data_maps/neoforge:block/waxables".to_owned(),
                layers(&[
                    json!({"values": {"test:tin": {
                        "waxed": "test:waxed_tin", "generate_block_transform": false
                    }}}),
                    json!({"values": {"test:tin": {}}}),
                    json!({"replace": true, "values": {
                        "minecraft:stone": {"waxed": "test:waxed_tin"},
                        "test:tin": {"waxed": "test:not_registered"}
                    }}),
                ]),
            ),
        ];
        let overlay = sunrise_world::loot::PackOverlay {
            files: &files,
            ..sunrise_world::loot::PackOverlay::default()
        };
        let ids = [
            "minecraft:stone",
            "test:tin",
            "test:weathered_tin",
            "test:waxed_tin",
        ]
        .map(String::from);
        let items = sunrise_items::items::Items::empty();
        let registries = WorldRegistries::new(
            [("minecraft:block", ids.as_slice())],
            None,
            &items,
            std::path::Path::new(""),
            &overlay,
        );
        let block_id = |name: &str| ids.iter().position(|id| id == name).map(|i| i as u32);
        let block_maps = load(&overlay, &registries, block_id).unwrap();
        assert_eq!(block_maps.flammable("minecraft:stone"), Some((5, 20)));
        assert_eq!(block_maps.waxed("minecraft:stone"), None);
        assert_eq!(
            block_maps.next_oxidation("test:tin"),
            Some("test:weathered_tin")
        );
        assert_eq!(block_maps.waxed("test:tin"), Some("test:waxed_tin"));
    }

    #[test]
    fn block_maps_publish_and_sync_real_rows_across_reload() {
        use sunrise_core::nbt::Tag;
        use sunrise_gameplay_common::game::native_data_maps_common::DataMaps;

        let file = |value: serde_json::Value| vec![serde_json::to_vec(&value).unwrap()];
        let ids = [
            "minecraft:stone",
            "test:tin",
            "test:weathered_tin",
            "test:waxed_tin",
        ]
        .map(String::from);
        let items = sunrise_items::items::Items::empty();
        let block_id = |name: &str| ids.iter().position(|id| id == name).map(|i| i as u32);

        // Initial load: all three built-in block maps carry non-empty rows.
        let initial_files = vec![
            (
                "data_maps/neoforge:block/flammable".to_owned(),
                file(json!({"values": {"minecraft:stone": {"ignite_odds": 5, "burn_odds": 20}}})),
            ),
            (
                "data_maps/neoforge:block/oxidizables".to_owned(),
                file(
                    json!({"values": {"test:tin": {"next_oxidation_stage": "test:weathered_tin"}}}),
                ),
            ),
            (
                "data_maps/neoforge:block/waxables".to_owned(),
                file(json!({"values": {"test:tin": {"waxed": "test:waxed_tin"}}})),
            ),
        ];
        let overlay = sunrise_world::loot::PackOverlay {
            files: &initial_files,
            ..sunrise_world::loot::PackOverlay::default()
        };
        let registries = WorldRegistries::new(
            [("minecraft:block", ids.as_slice())],
            None,
            &items,
            std::path::Path::new(""),
            &overlay,
        );
        let loaded = load(&overlay, &registries, block_id);
        let published = DataMaps::default().with_block_maps(loaded.as_deref());
        let snapshot = published.synced_snapshot();
        let block_sync = snapshot
            .registries
            .get("minecraft:block")
            .expect("minecraft:block synced rows");
        assert_eq!(
            block_sync.iter().find(|(id, _)| id == "neoforge:flammable"),
            Some(&(
                "neoforge:flammable".to_owned(),
                vec![(
                    "minecraft:stone".to_owned(),
                    Tag::compound([("ignite_odds", Tag::Int(5)), ("burn_odds", Tag::Int(20))]),
                )],
            ))
        );
        assert_eq!(
            block_sync
                .iter()
                .find(|(id, _)| id == "neoforge:oxidizables"),
            Some(&(
                "neoforge:oxidizables".to_owned(),
                vec![(
                    "test:tin".to_owned(),
                    Tag::compound([(
                        "next_oxidation_stage",
                        Tag::String("test:weathered_tin".into()),
                    )]),
                )],
            ))
        );
        assert_eq!(
            block_sync.iter().find(|(id, _)| id == "neoforge:waxables"),
            Some(&(
                "neoforge:waxables".to_owned(),
                vec![(
                    "test:tin".to_owned(),
                    Tag::compound([("waxed", Tag::String("test:waxed_tin".into()))]),
                )],
            ))
        );

        // Reload: replace waxables target and drop flammable/oxidizables.
        let reloaded_files = vec![(
            "data_maps/neoforge:block/waxables".to_owned(),
            file(json!({"values": {"minecraft:stone": {"waxed": "test:waxed_tin"}}})),
        )];
        let reloaded_overlay = sunrise_world::loot::PackOverlay {
            files: &reloaded_files,
            ..sunrise_world::loot::PackOverlay::default()
        };
        let reloaded = load(&reloaded_overlay, &registries, block_id);
        let republished = DataMaps::default().with_block_maps(reloaded.as_deref());
        let reloaded_snapshot = republished.synced_snapshot();
        let reloaded_block = reloaded_snapshot
            .registries
            .get("minecraft:block")
            .expect("reloaded minecraft:block synced rows");
        assert_eq!(
            reloaded_block
                .iter()
                .find(|(id, _)| id == "neoforge:flammable"),
            Some(&("neoforge:flammable".to_owned(), Vec::new()))
        );
        assert_eq!(
            reloaded_block
                .iter()
                .find(|(id, _)| id == "neoforge:oxidizables"),
            Some(&("neoforge:oxidizables".to_owned(), Vec::new()))
        );
        assert_eq!(
            reloaded_block
                .iter()
                .find(|(id, _)| id == "neoforge:waxables"),
            Some(&(
                "neoforge:waxables".to_owned(),
                vec![(
                    "minecraft:stone".to_owned(),
                    Tag::compound([("waxed", Tag::String("test:waxed_tin".into()))]),
                )],
            ))
        );
    }
}
