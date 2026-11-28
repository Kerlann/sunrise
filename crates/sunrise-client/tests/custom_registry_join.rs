//! Configuration-phase sync of mod-declared `sync(true)` static custom registries over a
//! real loopback connection (`Lan` + `Hub` + `net::Session`), matching NeoForge's
//! `SyncRegistries`, `RegistryManager`, `ConfigurationInitialization` and `ClientPayloadHandler`.
extern crate sunrise_client_lib as sunrise_client;
#[path = "support/loopback.rs"]
mod loopback;

use loopback::{Host, Resources, connect_vanilla_to_config};
use mod_api::Identifier;
use std::{
    any::Any,
    sync::Arc,
    time::{Duration, Instant},
};
use sunrise_client::{
    custom_registry_net::{
        self, LocalRegistries, Snapshot, SnapshotRegistry, SyncedCustomRegistries,
    },
    data_map_net::LocalDataMaps,
    lan::{Lan, NetConfig, data::SyncData},
    net::{Event, LocalInventory, Session},
};
use sunrise_protocol::packets::{
    Empty,
    configuration::{self as cfg, RegistryTags, TagEntry},
};
use sunrise_world::item_defaults::ItemData;

struct LocalDir {
    id: String,
    tag_dir: String,
    sync: bool,
    entries: Vec<Identifier>,
}

impl LocalDir {
    fn arc(
        id: &str,
        sync: bool,
        entries: &[&str],
    ) -> Arc<dyn mod_api::__private::RegistryDirectory> {
        let parsed = Identifier::parse_namespaced(id).unwrap();
        Arc::new(Self {
            id: id.to_owned(),
            tag_dir: format!("{}/{}", parsed.namespace(), parsed.path()),
            sync,
            entries: entries
                .iter()
                .map(|entry| Identifier::parse_namespaced(entry).unwrap())
                .collect(),
        })
    }
}

impl mod_api::__private::RegistryDirectory for LocalDir {
    fn id(&self) -> &str {
        &self.id
    }
    fn sync(&self) -> bool {
        self.sync
    }
    fn entry_ids(&self) -> &[Identifier] {
        &self.entries
    }
    fn contains_id(&self, id: &str) -> bool {
        self.entries.iter().any(|entry| entry.to_string() == id)
    }
    fn tag_directory(&self) -> &str {
        &self.tag_dir
    }
    fn as_any(&self) -> &(dyn Any + Send + Sync) {
        self
    }
}

fn connect(
    port: u16,
    name: &str,
    data: &Arc<ItemData>,
    local_registries: LocalRegistries,
) -> Session {
    loopback::connect(
        port,
        name,
        data,
        LocalInventory {
            custom_registries: local_registries,
            ..Default::default()
        },
    )
}

fn drive_join(
    host: &mut Host,
    session: &Session,
) -> Result<
    (
        Arc<sunrise_protocol::registry::Dynamic>,
        SyncedCustomRegistries,
    ),
    String,
> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        host.tick();
        for receipt in session.poll() {
            let (event, _credit) = receipt.into_parts();
            let event = match event {
                Event::Configuration(task) => {
                    let Some(effect) = task.handle() else {
                        continue;
                    };
                    effect
                }
                Event::Cookie(task) => {
                    task.handle();
                    continue;
                }
                Event::FinishConfiguration(change) => {
                    change.acknowledge();
                    continue;
                }
                event => event,
            };
            match event {
                Event::Joined {
                    registries,
                    custom_registries,
                    ..
                } => return Ok((registries, custom_registries)),
                Event::Disconnected { reason, .. } => return Err(reason),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for join");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Matching synced registries join cleanly across differing registration orders,
/// extra client-only entries/registries and empty absent server registries are accepted,
/// and `UpdateTags` decodes custom registry tags via the server's raw ID order.
#[test]
#[ignore = "daily"]
fn matching_synced_registries_join_and_decode_tags_across_differing_orders() {
    let _publication = custom_registry_net::isolate_server_publication();
    custom_registry_net::serve(Some(Arc::new(Snapshot {
        registries: vec![
            SnapshotRegistry {
                registry_id: "chemlib:gas".into(),
                entries: vec!["boiler:steam".into(), "chemlib:oxygen".into()],
            },
            SnapshotRegistry {
                registry_id: "chemlib:empty".into(),
                entries: vec![],
            },
        ],
    })));
    let local = LocalRegistries::new(vec![
        // Client registered oxygen (0) before steam (1), plus an extra client-only entry.
        LocalDir::arc(
            "chemlib:gas",
            true,
            &["chemlib:oxygen", "boiler:steam", "clientonly:neon"],
        ),
        LocalDir::arc("clientonly:extra", true, &["clientonly:a"]),
    ]);

    let mut sync = SyncData::empty();
    // Server tag `boiler:hot` contains server raw_id 0 (`boiler:steam`).
    sync.custom_registry_tags.push(RegistryTags {
        registry: "chemlib:gas".into(),
        tags: vec![TagEntry {
            name: "boiler:hot".into(),
            ids: vec![0],
        }],
    });
    let (_resources, data) = Resources::new("custom-registry-join", "matching");
    let mut host = Host::start(sync, "Custom registry join", LocalDataMaps::default());
    let session = connect(host.port(), "Alchemist", &data, local);
    let (registries, synced) = drive_join(&mut host, &session).expect("matching client joins");

    // Server raw_id 0 decodes to `boiler:steam` despite client having oxygen at slot 0.
    let raw_ids = registries.tag("chemlib:gas", "boiler:hot").unwrap();
    let decoded: Vec<String> = raw_ids
        .iter()
        .map(|&raw_id| synced.entry_id("chemlib:gas", raw_id).unwrap().to_string())
        .collect();
    assert_eq!(decoded, vec!["boiler:steam"]);
}

/// Client missing a non-empty synced registry disconnects naming the registry
/// (`RegistryManager.applySnapshot:147-149`, `ClientPayloadHandler.java:114-118`).
#[test]
#[ignore = "daily"]
fn client_missing_non_empty_synced_registry_disconnects_naming_registry() {
    let _publication = custom_registry_net::isolate_server_publication();
    custom_registry_net::serve(Some(Arc::new(Snapshot {
        registries: vec![SnapshotRegistry {
            registry_id: "chemlib:gas".into(),
            entries: vec!["chemlib:oxygen".into()],
        }],
    })));
    let local = LocalRegistries::new(vec![]);

    let (_resources, data) = Resources::new("custom-registry-join", "missing-reg");
    let mut host = Host::start(
        SyncData::empty(),
        "Custom registry join",
        LocalDataMaps::default(),
    );
    let session = connect(host.port(), "PlainSunrise", &data, local);
    let reason = drive_join(&mut host, &session).unwrap_err();
    assert!(
        reason.contains("chemlib:gas"),
        "expected disconnect naming chemlib:gas, got: {reason}"
    );
}

/// Client missing an entry sent by the server disconnects naming the registry and unknown entry
/// (`RegistryManager.applySnapshot:176-179`, `ClientPayloadHandler.java:104-110`).
#[test]
#[ignore = "daily"]
fn client_missing_synced_entry_disconnects_naming_registry_and_entry() {
    let _publication = custom_registry_net::isolate_server_publication();
    custom_registry_net::serve(Some(Arc::new(Snapshot {
        registries: vec![SnapshotRegistry {
            registry_id: "chemlib:gas".into(),
            entries: vec!["chemlib:oxygen".into(), "boiler:steam".into()],
        }],
    })));
    let local = LocalRegistries::new(vec![LocalDir::arc(
        "chemlib:gas",
        true,
        &["chemlib:oxygen"],
    )]);

    let (_resources, data) = Resources::new("custom-registry-join", "missing-entry");
    let mut host = Host::start(
        SyncData::empty(),
        "Custom registry join",
        LocalDataMaps::default(),
    );
    let session = connect(host.port(), "HalfModded", &data, local);
    let reason = drive_join(&mut host, &session).unwrap_err();
    assert!(
        reason.contains("chemlib:gas") && reason.contains("boiler:steam"),
        "expected disconnect naming chemlib:gas and boiler:steam, got: {reason}"
    );
}

/// Vanilla client joining a server with `sync(true)` custom registries is never disconnected
/// and receives neither `sunrise:custom_registries` nor custom-registry `UpdateTags` groups
/// (`ConfigurationInitialization.configureEarlyTasks:38-42`, `NetworkInitialization.java:42-58`).
#[test]
#[ignore = "daily"]
fn vanilla_client_joins_without_custom_registry_payload_or_disconnect() {
    let _publication = custom_registry_net::isolate_server_publication();
    custom_registry_net::serve(Some(Arc::new(Snapshot {
        registries: vec![SnapshotRegistry {
            registry_id: "chemlib:gas".into(),
            entries: vec!["chemlib:oxygen".into()],
        }],
    })));
    let mut sync = SyncData::empty();
    sync.custom_registry_tags.push(RegistryTags {
        registry: "chemlib:gas".into(),
        tags: vec![TagEntry {
            name: "boiler:hot".into(),
            ids: vec![0],
        }],
    });
    let net = NetConfig {
        announce: false,
        ..NetConfig::loopback(0, "Vanilla custom registry join".into(), 8)
    };
    let lan = Lan::start_with(Arc::new(sync), net).unwrap();
    let mut c = connect_vanilla_to_config(lan.port, "Vanilla");
    loop {
        let (id, payload) = c.recv().unwrap();
        match cfg::Clientbound::decode(id, &payload).unwrap() {
            cfg::Clientbound::SelectKnownPacks(_) => c
                .send(
                    &cfg::Serverbound::SelectKnownPacks(cfg::SelectKnownPacks { packs: vec![] })
                        .encode(),
                )
                .unwrap(),
            cfg::Clientbound::CustomPayload(p) if p.channel == custom_registry_net::CHANNEL => {
                panic!("vanilla client must not receive sunrise:custom_registries");
            }
            cfg::Clientbound::UpdateTags(tags) => {
                assert!(
                    !tags.registries.iter().any(|r| r.registry == "chemlib:gas"),
                    "vanilla client must not receive custom registry tags in UpdateTags"
                );
            }
            cfg::Clientbound::Disconnect(d) => {
                panic!("vanilla client must not be disconnected: {:?}", d.reason);
            }
            cfg::Clientbound::FinishConfiguration(_) => {
                c.send(&cfg::Serverbound::FinishConfiguration(Empty).encode())
                    .unwrap();
                break;
            }
            _ => {}
        }
    }
}
