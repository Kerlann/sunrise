//! Configuration negotiation and play-phase synchronization for synced data maps
//! (`sunrise:data_map_known`, `sunrise:data_map_known_reply`, `sunrise:data_map_sync`),
//! matching NeoForge's `RegistryDataMapNegotiation`, `ClientRegistryManager`,
//! `RegistryManager.handleKnownDataMapsReply`, and `RegistryDataMapSyncPayload`.
use mod_api::DataMapSync;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex},
};
use sunrise_core::{nbt::Tag, vanilla_bytes::Writer};
use sunrise_protocol::{
    buf::Reader,
    packets::{configuration as cfg, play},
};

/// Configuration S->C channel listing every synced (`Optional` or `Required`) data map on
/// the server (`KnownRegistryDataMapsPayload`).
pub const KNOWN_CHANNEL: &str = "sunrise:data_map_known";

/// Configuration C->S channel listing every synced data map the client declared
/// (`KnownRegistryDataMapsReplyPayload`).
pub const KNOWN_REPLY_CHANNEL: &str = "sunrise:data_map_known_reply";

/// Play S->C channel syncing one registry's data maps on initial join and every `/reload`
/// (`RegistryDataMapSyncPayload`).
pub const SYNC_CHANNEL: &str = "sunrise:data_map_sync";

/// Built-in synced NeoForge data maps (`NeoForgeDataMaps.java`, all `.synced(..., false)` /
/// `Optional`).
pub const BUILTIN_SYNCED_MAPS: &[(&str, &str)] = &[
    ("minecraft:block", "neoforge:flammable"),
    ("minecraft:block", "neoforge:oxidizables"),
    ("minecraft:block", "neoforge:waxables"),
    (
        "minecraft:entity_type",
        "neoforge:acceptable_villager_distances",
    ),
    ("minecraft:entity_type", "neoforge:parrot_imitations"),
    ("minecraft:worldgen/biome", "neoforge:villager_types"),
    ("minecraft:villager_profession", "neoforge:raid_hero_gifts"),
    ("minecraft:game_event", "neoforge:vibration_frequencies"),
    ("minecraft:item", "neoforge:villager_compostables"),
];

/// Vanilla string length limit (`FriendlyByteBuf.DEFAULT_MAX_STRING_LENGTH`).
const MAX_STRING_LEN: usize = 32767;

/// One advertised synced data map in `sunrise:data_map_known`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KnownEntry {
    pub registry: String,
    pub map_id: String,
    pub mandatory: bool,
}

/// Synced data-map inventory captured per server listener or client connection (never a
/// process-global test override).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalDataMaps {
    entries: Arc<[KnownEntry]>,
}

impl LocalDataMaps {
    /// Snapshot the installed built-in and startup-registered synced data maps.
    pub fn installed() -> Self {
        Self {
            entries: installed_synced_entries().into(),
        }
    }

    /// Build a custom per-connection synced data-map inventory for loopback tests.
    #[cfg(any(test, feature = "test-publication-scope"))]
    pub fn from_entries(mut entries: Vec<KnownEntry>) -> Self {
        entries.sort();
        entries.dedup_by(|a, b| a.registry == b.registry && a.map_id == b.map_id);
        Self {
            entries: entries.into(),
        }
    }

    /// All advertised synced (`Optional` and `Required`) maps in canonical sorted order.
    pub fn entries(&self) -> &[KnownEntry] {
        &self.entries
    }

    /// Whether any advertised map is `DataMapSync::Required` (`mandatory = true`).
    pub fn has_mandatory(&self) -> bool {
        self.entries.iter().any(|entry| entry.mandatory)
    }

    /// Whether `(registry, map_id)` is in this inventory.
    pub fn contains(&self, registry: &str, map_id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.registry == registry && entry.map_id == map_id)
    }
}

/// Maps a client advertised in `sunrise:data_map_known_reply`, stored per live connection
/// (mirroring NeoForge's `RegistryManager.ATTRIBUTE_KNOWN_DATA_MAPS`).
pub type KnownDataMaps = BTreeMap<String, BTreeSet<String>>;

/// Server-published, network-projected snapshot of all synced data maps, grouped by
/// registry ID, then `(map_id, sorted (entry_id, projected Tag))`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncedDataMapsSnapshot {
    pub registries: BTreeMap<String, Vec<(String, Vec<(String, Tag)>)>>,
}

type Held = Mutex<Option<Arc<SyncedDataMapsSnapshot>>>;
static SERVED: Held = Mutex::new(None);

#[cfg(any(test, feature = "test-publication-scope"))]
thread_local! {
    static SCOPED_SERVED: std::cell::RefCell<Option<Option<Arc<SyncedDataMapsSnapshot>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Isolates this thread's data-map server publication for tests.
#[cfg(any(test, feature = "test-publication-scope"))]
pub fn isolate_server_publication() -> impl Drop {
    struct Scope(
        Option<Option<Arc<SyncedDataMapsSnapshot>>>,
        std::marker::PhantomData<std::rc::Rc<()>>,
    );
    impl Drop for Scope {
        fn drop(&mut self) {
            SCOPED_SERVED.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let previous = SCOPED_SERVED.with(|slot| slot.replace(Some(None)));
    Scope(previous, std::marker::PhantomData)
}

/// Publish the server's latest network-projected synced data-map snapshot (on world open
/// and every `/reload`), or clear it (`None`) on world close.
pub fn serve(snapshot: Option<Arc<SyncedDataMapsSnapshot>>) {
    #[cfg(any(test, feature = "test-publication-scope"))]
    if SCOPED_SERVED.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(held) = slot.as_mut() {
            *held = snapshot.clone();
            true
        } else {
            false
        }
    }) {
        return;
    }
    *SERVED.lock().unwrap_or_else(|e| e.into_inner()) = snapshot;
}

/// Read the currently served synced data-map snapshot.
pub fn served() -> Option<Arc<SyncedDataMapsSnapshot>> {
    #[cfg(any(test, feature = "test-publication-scope"))]
    if let Some(held) = SCOPED_SERVED.with(|slot| slot.borrow().clone()) {
        return held;
    }
    SERVED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn installed_synced_entries() -> Vec<KnownEntry> {
    let mut entries: Vec<KnownEntry> = BUILTIN_SYNCED_MAPS
        .iter()
        .map(|&(registry, map_id)| KnownEntry {
            registry: registry.to_owned(),
            map_id: map_id.to_owned(),
            mandatory: false,
        })
        .collect();
    for map in mod_api::__private::installed_data_maps() {
        let sync = map.sync();
        if sync == DataMapSync::ServerOnly {
            continue;
        }
        entries.push(KnownEntry {
            registry: map.registry().to_owned(),
            map_id: map.id_str().to_owned(),
            mandatory: sync == DataMapSync::Required,
        });
    }
    entries.sort();
    entries.dedup_by(|a, b| a.registry == b.registry && a.map_id == b.map_id);
    entries
}

/// If the server has any `DataMapSync::Required` (`mandatory`) data map, returns the plain
/// disconnect reason for vanilla / non-Sunrise clients (`RegistryDataMapNegotiation.run:37-50`).
pub fn refuse_vanilla_reason(server_local: &LocalDataMaps) -> Option<String> {
    let mandatory: Vec<String> = server_local
        .entries()
        .iter()
        .filter(|entry| entry.mandatory)
        .map(|entry| format!("{} ({})", entry.map_id, entry.registry))
        .collect();
    if mandatory.is_empty() {
        None
    } else {
        Some(format!(
            "This server does not support vanilla clients as it has mandatory registry data maps: {}",
            mandatory.join(", ")
        ))
    }
}

/// Server-side verification of a Sunrise client's `sunrise:data_map_known_reply`: every
/// server-mandatory map must appear in the reply (`RegistryManager.handleKnownDataMapsReply`).
pub fn validate_client_reply(
    server_local: &LocalDataMaps,
    reply: &KnownDataMaps,
) -> Result<(), String> {
    let missing: Vec<String> = server_local
        .entries()
        .iter()
        .filter(|entry| entry.mandatory)
        .filter(|entry| {
            !reply
                .get(&entry.registry)
                .is_some_and(|maps| maps.contains(&entry.map_id))
        })
        .map(|entry| format!("{} ({})", entry.map_id, entry.registry))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "This client is missing mandatory registry data maps required by the server: {}",
            missing.join(", ")
        ))
    }
}

/// Encode `sunrise:data_map_known` (S->C).
pub fn encode_known(entries: &[KnownEntry]) -> Result<Vec<u8>, String> {
    let mut w = Writer::new();
    let count = i32::try_from(entries.len())
        .map_err(|_| "Too many synced data maps to advertise.".to_owned())?;
    w.varint(count);
    for entry in entries {
        w.string(&entry.registry);
        w.string(&entry.map_id);
        w.bool(entry.mandatory);
    }
    Ok(w.data)
}

/// Decode `sunrise:data_map_known` (S->C).
pub fn decode_known(bytes: &[u8]) -> Result<Vec<KnownEntry>, String> {
    let mut r = Reader::new(bytes);
    let count = r.varint().map_err(|e| e.to_string())?;
    if count < 0 {
        return Err("Negative data map count in known payload.".into());
    }
    let count = count as usize;
    let mut entries = Vec::with_capacity(count.min(256));
    for _ in 0..count {
        let registry = r.string(MAX_STRING_LEN).map_err(|e| e.to_string())?;
        let map_id = r.string(MAX_STRING_LEN).map_err(|e| e.to_string())?;
        let mandatory = r.bool().map_err(|e| e.to_string())?;
        entries.push(KnownEntry {
            registry,
            map_id,
            mandatory,
        });
    }
    if r.remaining() != 0 {
        return Err("Trailing bytes in sunrise:data_map_known payload.".into());
    }
    Ok(entries)
}

/// Encode `sunrise:data_map_known_reply` (C->S).
pub fn encode_known_reply(known: &KnownDataMaps) -> Result<Vec<u8>, String> {
    let mut w = Writer::new();
    let reg_count = i32::try_from(known.len())
        .map_err(|_| "Too many registries in known data maps reply.".to_owned())?;
    w.varint(reg_count);
    for (registry, map_ids) in known {
        w.string(registry);
        let map_count = i32::try_from(map_ids.len())
            .map_err(|_| format!("Too many data maps for registry {registry}."))?;
        w.varint(map_count);
        for map_id in map_ids {
            w.string(map_id);
        }
    }
    Ok(w.data)
}

/// Decode `sunrise:data_map_known_reply` (C->S).
pub fn decode_known_reply(bytes: &[u8]) -> Result<KnownDataMaps, String> {
    let mut r = Reader::new(bytes);
    let reg_count = r.varint().map_err(|e| e.to_string())?;
    if reg_count < 0 {
        return Err("Negative registry count in known data maps reply.".into());
    }
    let mut known = KnownDataMaps::new();
    for _ in 0..(reg_count as usize) {
        let registry = r.string(MAX_STRING_LEN).map_err(|e| e.to_string())?;
        let map_count = r.varint().map_err(|e| e.to_string())?;
        if map_count < 0 {
            return Err("Negative data map count in known data maps reply.".into());
        }
        let mut set = BTreeSet::new();
        for _ in 0..(map_count as usize) {
            set.insert(r.string(MAX_STRING_LEN).map_err(|e| e.to_string())?);
        }
        known.insert(registry, set);
    }
    if r.remaining() != 0 {
        return Err("Trailing bytes in sunrise:data_map_known_reply payload.".into());
    }
    Ok(known)
}

/// Client handler for `sunrise:data_map_known` (`ClientRegistryManager.handleKnownDataMaps:56-105`).
///
/// Compares client and server mandatory (`DataMapSync::Required`) sets; disconnects naming
/// any mismatches. Otherwise returns the encoded `sunrise:data_map_known_reply` listing
/// every synced (`Optional` + `Required`) map the client knows.
pub fn handle_known_data_maps(
    server_payload: &[u8],
    client_local: &LocalDataMaps,
) -> Result<Vec<u8>, String> {
    let server_entries = decode_known(server_payload)?;
    let client_entries = client_local.entries();

    let our_mandatory: BTreeSet<(String, String)> = client_entries
        .iter()
        .filter(|e| e.mandatory)
        .map(|e| (e.registry.clone(), e.map_id.clone()))
        .collect();
    let their_mandatory: BTreeSet<(String, String)> = server_entries
        .iter()
        .filter(|e| e.mandatory)
        .map(|e| (e.registry.clone(), e.map_id.clone()))
        .collect();

    let mut messages = Vec::new();
    let missing_our: Vec<String> = our_mandatory
        .difference(&their_mandatory)
        .map(|(reg, id)| format!("{id} ({reg})"))
        .collect();
    if !missing_our.is_empty() {
        messages.push(format!(
            "The server is missing mandatory registry data maps required by this client: {}",
            missing_our.join(", ")
        ));
    }
    let missing_their: Vec<String> = their_mandatory
        .difference(&our_mandatory)
        .map(|(reg, id)| format!("{id} ({reg})"))
        .collect();
    if !missing_their.is_empty() {
        messages.push(format!(
            "This client is missing mandatory registry data maps required by the server: {}",
            missing_their.join(", ")
        ));
    }
    if !messages.is_empty() {
        return Err(messages.join("\n"));
    }

    let mut known = KnownDataMaps::new();
    for entry in client_entries {
        known
            .entry(entry.registry.clone())
            .or_default()
            .insert(entry.map_id.clone());
    }
    encode_known_reply(&known)
}

/// Encode `sunrise:data_map_sync` (S->C in play) for one registry
/// (`RegistryDataMapSyncPayload`).
///
/// Entries whose projected NBT cannot be encoded on the wire (including `Tag::End`,
/// strings exceeding modified UTF-8 limits, or tags exceeding NBT depth limits) are
/// logged once per map and omitted rather than substituting `Tag::End`.
pub fn encode_sync_payload(
    registry: &str,
    maps: &[(String, Vec<(String, Tag)>)],
) -> Result<Vec<u8>, String> {
    let mut w = Writer::new();
    w.string(registry);
    let map_count = i32::try_from(maps.len())
        .map_err(|_| format!("Too many data maps for registry {registry}."))?;
    w.varint(map_count);
    let limits = sunrise_core::nbt::Limits::default();
    for (map_id, entries) in maps {
        let mut encoded_entries: Vec<(&str, Vec<u8>)> = Vec::with_capacity(entries.len());
        let mut warned = false;
        for (entry_id, tag) in entries {
            let encoded = if matches!(tag, Tag::End) {
                Err("Tag::End cannot be synced as a data map value".to_owned())
            } else {
                sunrise_core::nbt::write_network(tag, limits).map_err(|error| error.to_string())
            };
            match encoded {
                Ok(bytes) => encoded_entries.push((entry_id.as_str(), bytes)),
                Err(error) => {
                    if !warned {
                        warned = true;
                        log::warn!(
                            "Sunrise: data map {map_id} on {registry}: failed to encode projected value for entry {entry_id} ({error}); omitting entry from client sync"
                        );
                    }
                }
            }
        }
        w.string(map_id);
        let entry_count = i32::try_from(encoded_entries.len())
            .map_err(|_| format!("Too many entries in data map {map_id}."))?;
        w.varint(entry_count);
        for (entry_id, nbt_bytes) in encoded_entries {
            w.string(entry_id);
            w.bytes(&nbt_bytes);
        }
    }
    Ok(w.data)
}

/// Decode `sunrise:data_map_sync` (S->C in play) for one registry
/// (`RegistryDataMapSyncPayload`). Entries in each map are sorted by entry ID.
///
/// Any malformed payload or `Tag::End` value fails the whole registry decode with an
/// error naming the registry (and map when known) so the client disconnects without
/// applying partial state.
pub fn decode_sync_payload(
    bytes: &[u8],
) -> Result<(String, HashMap<String, Vec<(String, Tag)>>), String> {
    let mut r = Reader::new(bytes);
    let registry = r
        .string(MAX_STRING_LEN)
        .map_err(|e| format!("invalid registry id in sunrise:data_map_sync: {e}"))?;
    let map_count = r
        .varint()
        .map_err(|e| format!("invalid map count for registry {registry}: {e}"))?;
    if map_count < 0 {
        return Err(format!(
            "negative map count for registry {registry} in sunrise:data_map_sync payload"
        ));
    }
    let mut maps = HashMap::with_capacity((map_count as usize).min(256));
    for _ in 0..(map_count as usize) {
        let map_id = r
            .string(MAX_STRING_LEN)
            .map_err(|e| format!("invalid map id for registry {registry}: {e}"))?;
        if maps.contains_key(&map_id) {
            return Err(format!(
                "duplicate data map {map_id} for registry {registry} in sunrise:data_map_sync payload"
            ));
        }
        let entry_count = r
            .varint()
            .map_err(|e| format!("invalid entry count for data map {map_id} ({registry}): {e}"))?;
        if entry_count < 0 {
            return Err(format!(
                "negative entry count for data map {map_id} ({registry}) in sunrise:data_map_sync payload"
            ));
        }
        let mut entries = Vec::with_capacity((entry_count as usize).min(4096));
        for _ in 0..(entry_count as usize) {
            let entry_id = r
                .string(MAX_STRING_LEN)
                .map_err(|e| format!("invalid entry id in data map {map_id} ({registry}): {e}"))?;
            let tag = r.nbt().map_err(|e| {
                format!(
                    "undecodable NBT in data map {map_id} ({registry}) for entry {entry_id}: {e}"
                )
            })?;
            if matches!(tag, Tag::End) {
                return Err(format!(
                    "unexpected Tag::End value in data map {map_id} ({registry}) for entry {entry_id}"
                ));
            }
            entries.push((entry_id, tag));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(pair) = entries.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(format!(
                "duplicate entry {} in data map {map_id} ({registry}) in sunrise:data_map_sync payload",
                pair[0].0
            ));
        }
        maps.insert(map_id, entries);
    }
    if r.remaining() != 0 {
        return Err(format!(
            "trailing bytes in sunrise:data_map_sync payload for registry {registry}"
        ));
    }
    Ok((registry, maps))
}

/// Send the latest synced data-map payloads to one live Sunrise connection, filtered to the
/// maps that connection advertised in `sunrise:data_map_known_reply`.
pub fn sync_connection(sender: &crate::lan::PlayerSender, server_local: &LocalDataMaps) {
    let Some(known) = sender.known_data_maps.as_ref() else {
        return;
    };
    let Some(snapshot) = served() else {
        return;
    };
    for (registry, client_maps) in known.iter() {
        if client_maps.is_empty() {
            continue;
        }
        let mut out_maps: Vec<(String, Vec<(String, Tag)>)> = Vec::new();
        if let Some(reg_maps) = snapshot.registries.get(registry) {
            for (map_id, entries) in reg_maps {
                if client_maps.contains(map_id) {
                    out_maps.push((map_id.clone(), entries.clone()));
                }
            }
        }
        // Always include every known map that exists on the server even when its entry list
        // is empty so a `/reload` that drops all entries clears the client's copy.
        for map_id in client_maps {
            if !out_maps.iter().any(|(id, _)| id == map_id)
                && server_local.contains(registry, map_id)
            {
                out_maps.push((map_id.clone(), Vec::new()));
            }
        }
        if out_maps.is_empty() {
            continue;
        }
        match encode_sync_payload(registry, &out_maps) {
            Ok(data) => {
                let packet = play::Clientbound::CustomPayload(cfg::CustomPayload {
                    channel: SYNC_CHANNEL.into(),
                    data,
                });
                sender.send(packet.encode());
            }
            Err(error) => {
                log::error!("Sunrise: failed to encode data map sync for {registry}: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mod_api::{DataMapSync, Identifier, RawDataMap};

    fn entry(registry: &str, map_id: &str, mandatory: bool) -> KnownEntry {
        KnownEntry {
            registry: registry.to_owned(),
            map_id: map_id.to_owned(),
            mandatory,
        }
    }

    #[test]
    fn network_value_projection_transforms_and_omits_failed_entries() {
        let reg = mod_api::__private::RegisteredDataMap::from_raw(
            "test",
            Identifier::parse_namespaced("minecraft:item").unwrap(),
            Identifier::parse_namespaced("test:projected").unwrap(),
            RawDataMap::new("minecraft:item", "test:projected")
                .sync(DataMapSync::Optional)
                .network_value(|tag| match tag {
                    Tag::Int(v) if *v >= 0 => Some(Tag::Int(v * 2)),
                    _ => None,
                }),
        );
        assert_eq!(
            reg.project_network_value("minecraft:coal", &Tag::Int(5)),
            Some(Tag::Int(10))
        );
        assert_eq!(
            reg.project_network_value("minecraft:stick", &Tag::Int(-1)),
            None
        );
    }

    #[test]
    fn sync_payload_round_trips_and_rejects_trailing_bytes() {
        let encoded = encode_sync_payload(
            "minecraft:item",
            &[(
                "chemlib:fuel".to_owned(),
                vec![
                    ("minecraft:stick".to_owned(), Tag::Int(100)),
                    ("minecraft:coal".to_owned(), Tag::Int(1600)),
                ],
            )],
        )
        .unwrap();
        let (registry, maps) = decode_sync_payload(&encoded).unwrap();
        assert_eq!(registry, "minecraft:item");
        assert_eq!(
            maps["chemlib:fuel"],
            vec![
                ("minecraft:coal".to_owned(), Tag::Int(1600)),
                ("minecraft:stick".to_owned(), Tag::Int(100)),
            ]
        );
        let mut with_trailing = encoded;
        with_trailing.push(0xAA);
        assert!(decode_sync_payload(&with_trailing).is_err());
    }

    #[test]
    fn unencodable_values_are_omitted_and_receiver_refuses_tag_end_or_corrupt_nbt() {
        let oversized = "x".repeat(70_000);
        let encoded = encode_sync_payload(
            "minecraft:item",
            &[(
                "chemlib:fuel".to_owned(),
                vec![
                    ("minecraft:coal".to_owned(), Tag::Int(1600)),
                    (
                        "minecraft:stick".to_owned(),
                        Tag::String(oversized.as_str().into()),
                    ),
                    ("minecraft:paper".to_owned(), Tag::End),
                ],
            )],
        )
        .unwrap();
        let (registry, maps) = decode_sync_payload(&encoded).unwrap();
        assert_eq!(registry, "minecraft:item");
        assert_eq!(
            maps["chemlib:fuel"],
            vec![("minecraft:coal".to_owned(), Tag::Int(1600))]
        );

        // Receiver must refuse Tag::End with an error naming both registry and map.
        let mut end_payload = Writer::new();
        end_payload.string("minecraft:item");
        end_payload.varint(1);
        end_payload.string("chemlib:fuel");
        end_payload.varint(1);
        end_payload.string("minecraft:stick");
        end_payload.u8(0);
        let err = decode_sync_payload(&end_payload.data).unwrap_err();
        assert!(
            err.contains("minecraft:item") && err.contains("chemlib:fuel"),
            "unexpected Tag::End error: {err}"
        );

        // Receiver must refuse truncated NBT with an error naming both registry and map.
        let mut truncated = Writer::new();
        truncated.string("minecraft:item");
        truncated.varint(1);
        truncated.string("chemlib:fuel");
        truncated.varint(1);
        truncated.string("minecraft:coal");
        truncated.u8(3); // TAG_Int with missing 4-byte payload
        let err = decode_sync_payload(&truncated.data).unwrap_err();
        assert!(
            err.contains("minecraft:item") && err.contains("chemlib:fuel"),
            "unexpected truncated NBT error: {err}"
        );

        // Receiver must refuse duplicate map ids naming registry and map.
        let dup_map = encode_sync_payload(
            "minecraft:item",
            &[
                (
                    "chemlib:fuel".to_owned(),
                    vec![("minecraft:coal".to_owned(), Tag::Int(1))],
                ),
                (
                    "chemlib:fuel".to_owned(),
                    vec![("minecraft:stick".to_owned(), Tag::Int(2))],
                ),
            ],
        )
        .unwrap();
        let err = decode_sync_payload(&dup_map).unwrap_err();
        assert!(
            err.contains("duplicate")
                && err.contains("minecraft:item")
                && err.contains("chemlib:fuel"),
            "unexpected duplicate map error: {err}"
        );

        // Receiver must refuse duplicate entry ids naming registry, map, and entry.
        let dup_entry = encode_sync_payload(
            "minecraft:item",
            &[(
                "chemlib:fuel".to_owned(),
                vec![
                    ("minecraft:coal".to_owned(), Tag::Int(1)),
                    ("minecraft:coal".to_owned(), Tag::Int(2)),
                ],
            )],
        )
        .unwrap();
        let err = decode_sync_payload(&dup_entry).unwrap_err();
        assert!(
            err.contains("duplicate")
                && err.contains("minecraft:item")
                && err.contains("chemlib:fuel")
                && err.contains("minecraft:coal"),
            "unexpected duplicate entry error: {err}"
        );
    }

    #[test]
    fn handle_known_data_maps_and_server_validation_enforce_mandatory_maps() {
        let server_req =
            LocalDataMaps::from_entries(vec![entry("minecraft:item", "chemlib:fuel", true)]);
        let client_empty = LocalDataMaps::from_entries(vec![]);
        let client_req =
            LocalDataMaps::from_entries(vec![entry("minecraft:item", "boiler:heat", true)]);

        let server_wire = encode_known(server_req.entries()).unwrap();
        let err = handle_known_data_maps(&server_wire, &client_empty).unwrap_err();
        assert!(err.contains("chemlib:fuel") && err.contains("minecraft:item"));

        let empty_wire = encode_known(&[]).unwrap();
        let err = handle_known_data_maps(&empty_wire, &client_req).unwrap_err();
        assert!(err.contains("boiler:heat") && err.contains("minecraft:item"));

        let err = validate_client_reply(&server_req, &KnownDataMaps::new()).unwrap_err();
        assert!(err.contains("chemlib:fuel") && err.contains("minecraft:item"));

        let mut ok_reply = KnownDataMaps::new();
        ok_reply
            .entry("minecraft:item".to_owned())
            .or_default()
            .insert("chemlib:fuel".to_owned());
        assert_eq!(validate_client_reply(&server_req, &ok_reply), Ok(()));
    }
}
