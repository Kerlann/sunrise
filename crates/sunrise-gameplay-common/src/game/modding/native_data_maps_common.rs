use crate::game::{
    data_map_files::{self, folder},
    data_map_json::to_tag,
};
use serde_json::Value;
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};
use sunrise_core::{identifier::Identifier, nbt::Tag};
use sunrise_sim::neoforge_mob_maps::{BuiltinMap, NeoForgeMobMaps};

/// The registries mods read through a typed key (`DataMapEntry`): always
/// known, even where the command registries list no ids.
const TYPED: [&str; 9] = [
    "minecraft:item",
    "minecraft:block",
    "minecraft:fluid",
    "minecraft:mob_effect",
    "minecraft:entity_type",
    "minecraft:enchantment",
    "minecraft:block_entity_type",
    "minecraft:villager_profession",
    "minecraft:worldgen/biome",
];

/// The registries a data load resolves maps against.
pub trait MapRegistries {
    /// Whether the game names `registry`: built in, a pack's or a datapack
    /// registry a mod declared.
    fn known(&self, registry: &str) -> bool;
    /// Whether `entry` is in `registry`; true where the registry's ids aren't listed.
    fn contains(&self, registry: &str, entry: &str) -> bool;
    /// The members of `tag` (`minecraft:logs`) in `registry`; none for an unknown tag.
    fn tag(&self, registry: &str, tag: &str) -> Vec<String>;
}

/// A world's registries while its server tables build: candidate IDs, the
/// declared registries' admitted entries, and the candidate's bound tags.
/// Standalone readers can also supply their own registry metadata and packs.
pub struct WorldRegistries<'a> {
    ids: BTreeMap<&'a str, &'a [String]>,
    biome_keys: Option<&'a [mod_api::BiomeKey]>,
    candidate: Option<&'a super::load_conditions::RegistryView>,
    datapack: Option<&'a mod_api::DatapackRegistrySet>,
    items: &'a sunrise_items::items::Items,
    bound_tags: Option<&'a PackTags>,
    root: &'a std::path::Path,
    overlay: &'a sunrise_world::loot::PackOverlay<'a>,
    id_sets: RefCell<HashMap<String, HashSet<String>>>,
    tags: RefCell<HashMap<String, HashMap<String, Vec<String>>>>,
}
pub type PackTags = sunrise_sim::lan::data::PackTags;
impl<'a> WorldRegistries<'a> {
    pub fn new(
        ids: impl IntoIterator<Item = (&'a str, &'a [String])>,
        datapack: Option<&'a mod_api::DatapackRegistrySet>,
        items: &'a sunrise_items::items::Items,
        root: &'a std::path::Path,
        overlay: &'a sunrise_world::loot::PackOverlay<'a>,
    ) -> Self {
        Self {
            ids: ids.into_iter().collect(),
            biome_keys: None,
            candidate: None,
            datapack,
            items,
            bound_tags: None,
            root,
            overlay,
            id_sets: RefCell::default(),
            tags: RefCell::default(),
        }
    }

    /// Resolve biome entries only against registered keys, not save placeholders.
    pub fn with_biome_keys(mut self, keys: &'a [mod_api::BiomeKey]) -> Self {
        self.biome_keys = Some(keys);
        self
    }

    /// Prefer the complete staged registry and tag bindings over command metadata.
    pub fn with_candidate(
        mut self,
        candidate: &'a super::load_conditions::RegistryView,
        tags: &'a PackTags,
    ) -> Self {
        self.candidate = Some(candidate);
        self.bound_tags = Some(tags);
        self
    }

    /// Whether a mod's declaration names `registry`.
    fn declared(&self, registry: &str) -> bool {
        mod_api::__private::datapack_registry_declarations()
            .iter()
            .any(|declaration| declaration.key == registry)
    }

    /// A frozen mod-declared static custom registry with `registry`, if any.
    fn custom_registry(
        &self,
        registry: &str,
    ) -> Option<&'static std::sync::Arc<dyn mod_api::__private::RegistryDirectory>> {
        mod_api::__private::custom_registry_directories()
            .iter()
            .find(|dir| dir.id() == registry)
    }
}
impl MapRegistries for WorldRegistries<'_> {
    fn known(&self, registry: &str) -> bool {
        self.candidate
            .is_some_and(|candidate| candidate.ids.contains_key(registry))
            || self.ids.contains_key(registry)
            || TYPED.contains(&registry)
            || self.declared(registry)
            || self.custom_registry(registry).is_some()
    }
    fn contains(&self, registry: &str, entry: &str) -> bool {
        if let Some(dir) = self.custom_registry(registry) {
            return dir.contains_id(entry);
        }
        if registry == sunrise_sim::lan::data::BIOME_REGISTRY
            && let Some(keys) = self.biome_keys
        {
            let mut sets = self.id_sets.borrow_mut();
            let set = sets
                .entry(registry.to_owned())
                .or_insert_with(|| keys.iter().map(ToString::to_string).collect());
            return set.contains(entry);
        }
        if self.declared(registry) {
            return self
                .datapack
                .is_some_and(|set| set.contains_entry(registry, entry));
        }
        if let Some(ids) = self
            .candidate
            .and_then(|candidate| candidate.ids.get(registry))
        {
            return ids.contains(entry);
        }
        if let Some(ids) = self.ids.get(registry).filter(|ids| !ids.is_empty()) {
            let mut sets = self.id_sets.borrow_mut();
            let set = sets
                .entry(registry.to_owned())
                .or_insert_with(|| ids.iter().cloned().collect());
            return set.contains(entry);
        }
        // A typed registry the sync did not list: no source says its entries.
        true
    }
    fn tag(&self, registry: &str, tag: &str) -> Vec<String> {
        use sunrise_items::items::TagRegistry;
        let ordered_or_sorted = |ordered: Option<&[String]>,
                                 fallback: &HashMap<String, HashSet<String>>|
         -> Vec<String> {
            if let Some(members) = ordered {
                return members.to_vec();
            }
            let mut members: Vec<String> = fallback
                .get(tag)
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default();
            members.sort();
            members
        };
        if self.declared(registry) {
            return self.datapack.map_or_else(Vec::new, |set| {
                mod_api::__private::game::datapack_tag_members(set, registry, tag).to_vec()
            });
        }
        match registry {
            "minecraft:item" => ordered_or_sorted(
                self.items.tag_members(TagRegistry::Item, tag),
                &self.items.item_tags,
            ),
            "minecraft:block" => ordered_or_sorted(
                self.items.tag_members(TagRegistry::Block, tag),
                &self.items.block_tags,
            ),
            "minecraft:fluid" => ordered_or_sorted(
                self.items.tag_members(TagRegistry::Fluid, tag),
                &self.items.fluid_tags,
            ),
            sunrise_items::items::EntityTypeTags::REGISTRY => ordered_or_sorted(
                self.items.tag_members(TagRegistry::EntityType, tag),
                self.items.entity_type_tags.members(),
            ),
            _ => {
                if let Some(tags) = self.bound_tags.and_then(|tags| {
                    tags.get(registry)
                        .or_else(|| tags.get(folder(registry).as_str()))
                }) {
                    return tags
                        .get(tag)
                        .map(|members| members.iter().cloned().collect())
                        .unwrap_or_default();
                }
                let mut loaded = self.tags.borrow_mut();
                let tags = loaded.entry(registry.to_owned()).or_insert_with(|| {
                    if let Some(dir) = self.custom_registry(registry) {
                        sunrise_world::tag_layers::load(
                            self.root,
                            dir.tag_directory(),
                            self.overlay,
                            |id| dir.contains_id(id),
                        )
                        .unwrap_or_else(|e| {
                            log::warn!("Sunrise: the {registry} tags data maps read: {e}");
                            HashMap::new()
                        })
                    } else {
                        sunrise_items::items::load_tags(self.root, &folder(registry), self.overlay)
                            .map(|loaded| {
                                loaded
                                    .into_iter()
                                    .map(|(tag_id, ids)| {
                                        let mut ordered: Vec<String> = ids.into_iter().collect();
                                        ordered.sort();
                                        (tag_id, ordered)
                                    })
                                    .collect()
                            })
                            .unwrap_or_else(|e| {
                                log::warn!("Sunrise: the {registry} tags data maps read: {e}");
                                HashMap::new()
                            })
                    }
                });
                tags.get(tag).cloned().unwrap_or_default()
            }
        }
    }
}

pub type LoadedMaps<V> = BTreeMap<String, HashMap<String, BTreeMap<String, V>>>;

#[derive(Default)]
pub struct DataMaps {
    pub biome_keys: std::sync::Arc<mod_api::__private::BiomeKeys>,
    maps: BTreeMap<String, HashMap<String, Vec<(String, Tag)>>>,
    pub mob_maps: std::sync::Arc<sunrise_sim::neoforge_mob_maps::NeoForgeMobMaps>,
}

/// `minecraft:` for an id written without a namespace.
pub(in crate::game) fn full_id(id: &str) -> Cow<'_, str> {
    if id.contains(':') {
        Cow::Borrowed(id)
    } else {
        Cow::Owned(format!("minecraft:{id}"))
    }
}

fn decode_entry_wrapper<V>(
    raw: &Value,
    decode: &impl Fn(&Value) -> Result<V, String>,
) -> Result<(bool, V), String> {
    // A direct object can have a field named `value`; only an object with
    // exactly the entry wrapper's fields is unwrapped. Conditions are already read.
    if let Value::Object(object) = raw
        && let Some(value) = object.get("value")
        && object.keys().all(|k| k == "value" || k == "replace")
    {
        let entry_replace = match object.get("replace") {
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("entry replace must be a boolean".into()),
            None => false,
        };
        return Ok((entry_replace, decode(value)?));
    }
    Ok((false, decode(raw)?))
}

/// Merge the registered maps' values from the server's tables' pack files, in pack order
/// (mods' below the packs'), with ids and tags resolved against `registries`. Unknown
/// registries are left out with a warning; a file that isn't a valid data map or contains
/// an undecodable value or remover is skipped atomically with an error.
pub fn load_maps<V: Clone, R>(
    registered: &[Arc<mod_api::__private::RegisteredDataMap>],
    overlay: &sunrise_world::loot::PackOverlay<'_>,
    registries: &dyn MapRegistries,
    decode_value: impl Fn(&str, &Value) -> Result<V, String>,
    decode_remover: impl Fn(&str, &Value) -> Result<R, String>,
    merge: impl Fn(&mod_api::__private::RegisteredDataMap, &str, &V, &V) -> Result<V, String>,
    remove_partial: impl Fn(
        &mod_api::__private::RegisteredDataMap,
        &str,
        &V,
        &R,
    ) -> Result<Option<V>, String>,
) -> LoadedMaps<V> {
    let mut maps: LoadedMaps<V> = BTreeMap::new();
    for reg in registered {
        let registry = reg.registry();
        let map_id = reg.id_str();
        if !registries.known(registry) {
            log::warn!(
                "Sunrise: data map {map_id} is for {registry}, a registry the game doesn't name: left out"
            );
            continue;
        }
        let by_map = maps.entry(registry.to_owned()).or_default();
        if by_map.contains_key(map_id) {
            log::warn!("Sunrise: data map {map_id} registered twice: the first is kept");
            continue;
        }
        let map = data_map_files::load_registered(
            reg.id(),
            registry,
            overlay,
            data_map_files::LayerCodecs {
                decode_value: |raw: &Value| decode_entry_wrapper(raw, &|v| decode_value(map_id, v)),
                decode_remover: |rem: &Value| decode_remover(map_id, rem),
            },
            data_map_files::LayerPolicy {
                supports_tags: reg.supports_tags(),
                tags: |tag: &str| registries.tag(registry, tag),
                known: |entry: &str| registries.contains(registry, entry),
                merge: |entry: &str, existing: &V, incoming: &V| {
                    merge(reg, entry, existing, incoming)
                },
                remove_partial: |entry: &str, existing: &V, remover: &R| {
                    remove_partial(reg, entry, existing, remover)
                },
            },
        );
        by_map.insert(map_id.to_owned(), map);
    }
    maps
}

pub struct Loaded {
    pub values: Vec<(BuiltinMap, BTreeMap<String, Tag>)>,
    pub snapshot: Arc<NeoForgeMobMaps>,
}

impl DataMaps {
    pub fn with_biome_keys(mut self, keys: mod_api::__private::BiomeKeys) -> Self {
        self.biome_keys = std::sync::Arc::new(keys);
        self
    }

    /// Merge the pack files and publish their winning values as NBT.
    pub fn load(
        registered: &[Arc<mod_api::__private::RegisteredDataMap>],
        overlay: &sunrise_world::loot::PackOverlay<'_>,
        registries: &dyn MapRegistries,
    ) -> Self {
        let merged = load_maps(
            registered,
            overlay,
            registries,
            |_, value| to_tag(value).map_err(|_| "value cannot be represented as NBT".to_owned()),
            |_, rem| to_tag(rem).map_err(|_| "remover cannot be represented as NBT".to_owned()),
            |reg, entry, existing, incoming| {
                let entry_id = Identifier::parse(entry)
                    .map_err(|e| format!("invalid entry id {entry:?}: {e}"))?;
                reg.merge(&entry_id, existing, incoming)
            },
            |reg, entry, existing, remover| {
                let entry_id = Identifier::parse(entry)
                    .map_err(|e| format!("invalid entry id {entry:?}: {e}"))?;
                reg.remove_partial(&entry_id, existing, remover)
            },
        );
        let mut published = Self::default();
        for (registry, maps) in merged {
            let mut types = HashMap::with_capacity(maps.len());
            for (id, values) in maps {
                types.insert(id, values.into_iter().collect());
            }
            published.maps.insert(registry, types);
        }
        published
    }

    /// Replace one registry's synced maps atomically on a remote client.
    pub fn replace_registry(
        &mut self,
        registry: String,
        maps: HashMap<String, Vec<(String, Tag)>>,
    ) {
        self.maps.insert(registry, maps);
    }

    /// Build the network-projected snapshot of all synced (`Optional` and `Required`) maps
    /// for server-to-client synchronization.
    pub fn synced_snapshot(&self) -> sunrise_sim::data_map_net::SyncedDataMapsSnapshot {
        let registered = mod_api::__private::installed_data_maps();
        let mut registries = BTreeMap::new();
        for (registry, by_map) in &self.maps {
            let mut out_maps = Vec::new();
            for (map_id, entries) in by_map {
                if let Some(reg) = registered
                    .iter()
                    .find(|r| r.registry() == registry && r.id_str() == map_id)
                {
                    if reg.sync() == mod_api::DataMapSync::ServerOnly {
                        continue;
                    }
                    let projected: Vec<(String, Tag)> = entries
                        .iter()
                        .filter_map(|(entry_id, tag)| {
                            reg.project_network_value(entry_id, tag)
                                .map(|projected| (entry_id.clone(), projected))
                        })
                        .collect();
                    out_maps.push((map_id.clone(), projected));
                } else if sunrise_sim::data_map_net::BUILTIN_SYNCED_MAPS
                    .contains(&(registry.as_str(), map_id.as_str()))
                {
                    out_maps.push((map_id.clone(), entries.clone()));
                }
            }
            if !out_maps.is_empty() {
                out_maps.sort_by(|a, b| a.0.cmp(&b.0));
                registries.insert(registry.clone(), out_maps);
            }
        }
        sunrise_sim::data_map_net::SyncedDataMapsSnapshot { registries }
    }

    pub fn with_builtins(mut self, builtin: Loaded) -> Self {
        for (definition, values) in builtin.values {
            if self
                .maps
                .entry(definition.registry.to_owned())
                .or_default()
                .insert(definition.id.to_owned(), values.into_iter().collect())
                .is_some()
            {
                log::warn!(
                    "Sunrise: builtin data map {} is reserved: engine definition kept",
                    definition.id
                );
            }
        }
        self.mob_maps = builtin.snapshot;
        self
    }

    /// Publish the resolved forward rows of NeoForge's built-in block maps (`neoforge:flammable`,
    /// `neoforge:oxidizables`, `neoforge:waxables`) alongside the other built-in data maps so
    /// server reads and `synced_snapshot` carry the real rows (or empty maps on removal).
    pub fn with_block_maps(mut self, block_maps: Option<&sunrise_world::level::BlockMaps>) -> Self {
        let by_block = self.maps.entry("minecraft:block".to_owned()).or_default();
        let entries = block_maps.map_or_else(
            sunrise_world::level::BlockMaps::empty_nbt_maps,
            sunrise_world::level::BlockMaps::nbt_maps,
        );
        for (id, values) in entries {
            by_block.insert(id.to_owned(), values);
        }
        self
    }

    /// The registries with a loaded type, in id order: what `data_maps_updated` names.
    pub fn registries(&self) -> Vec<String> {
        self.maps.keys().cloned().collect()
    }

    /// Map `type_id` of `registry`, refused where no mod registered it there.
    fn map(
        &self,
        registry: &str,
        type_id: &str,
    ) -> Result<&[(String, Tag)], mod_api::DataMapError> {
        self.maps
            .get(registry)
            .and_then(|maps| maps.get(type_id))
            .map(Vec::as_slice)
            .ok_or(mod_api::DataMapError::UnknownType)
    }

    /// `entry`'s value in map `type_id` of `registry`, as NBT: none where the
    /// map gives it none.
    pub fn value(
        &self,
        registry: &str,
        type_id: &str,
        entry: &str,
    ) -> Result<Option<&Tag>, mod_api::DataMapError> {
        let rows = self.map(registry, type_id)?;
        match rows.binary_search_by(|(id, _)| id.as_str().cmp(entry)) {
            Ok(index) => Ok(Some(&rows[index].1)),
            Err(_) => Ok(None),
        }
    }

    /// Every entry map `type_id` of `registry` gives a value, in id order.
    pub fn entries(
        &self,
        registry: &str,
        type_id: &str,
    ) -> Result<&[(String, Tag)], mod_api::DataMapError> {
        self.map(registry, type_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_tag_expansion_determines_which_rows_survive_merger_panic() {
        let run = |tag_order: Vec<String>| {
            let mut items = sunrise_items::items::Items::empty();
            items
                .item_tag_order
                .insert("test:fuels".to_owned(), tag_order);
            let files = vec![(
                "data_maps/test:item/order_panic".to_owned(),
                vec![
                    br#"{"values":{"minecraft:coal":1,"minecraft:stick":2}}"#.to_vec(),
                    br##"{"values":{"#test:fuels":10}}"##.to_vec(),
                ],
            )];
            let overlay = sunrise_world::loot::PackOverlay {
                files: &files,
                ..sunrise_world::loot::PackOverlay::default()
            };
            let ids = ["minecraft:coal", "minecraft:stick"].map(String::from);
            let registries = WorldRegistries::new(
                [("minecraft:item", ids.as_slice())],
                None,
                &items,
                std::path::Path::new(""),
                &overlay,
            );
            let reg = Arc::new(mod_api::__private::RegisteredDataMap::from_raw(
                "test",
                Identifier::parse_namespaced("minecraft:item").unwrap(),
                Identifier::parse_namespaced("test:order_panic").unwrap(),
                mod_api::RawDataMap::new("minecraft:item", "test:order_panic").merge(
                    |entry, existing, incoming| {
                        if entry.to_string() == "minecraft:stick" {
                            panic!("stick merger fails");
                        }
                        match (existing, incoming) {
                            (Tag::Int(a), Tag::Int(b)) => Ok(Tag::Int(a + b)),
                            _ => Ok(incoming.clone()),
                        }
                    },
                ),
            ));
            let published = DataMaps::load(&[reg], &overlay, &registries);
            (
                published
                    .value("minecraft:item", "test:order_panic", "minecraft:coal")
                    .unwrap()
                    .cloned(),
                published
                    .value("minecraft:item", "test:order_panic", "minecraft:stick")
                    .unwrap()
                    .cloned(),
            )
        };

        // coal before stick: coal merges (1 + 10 = 11) before stick panics (keeps 2).
        assert_eq!(
            run(vec!["minecraft:coal".into(), "minecraft:stick".into()]),
            (Some(Tag::Int(11)), Some(Tag::Int(2)))
        );
        // stick before coal: stick panics first (keeps 2) and poisons the merger before coal (keeps 1).
        assert_eq!(
            run(vec!["minecraft:stick".into(), "minecraft:coal".into()]),
            (Some(Tag::Int(1)), Some(Tag::Int(2)))
        );
    }
}
