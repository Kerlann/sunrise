//! Configuration negotiation and play-phase synchronization of synced data maps over a real
//! loopback connection (`Lan` + `Hub` + `net::Session`), matching NeoForge's
//! `RegistryDataMapNegotiation`, `ClientRegistryManager`, and `RegistryDataMapSyncPayload`.
extern crate sunrise_client_lib as sunrise_client;
#[path = "support/loopback.rs"]
mod loopback;

use loopback::{Host, Resources, connect_vanilla_to_config};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};
use sunrise_client::{
    data_map_net::{self, KnownEntry, LocalDataMaps, SyncedDataMapsSnapshot},
    lan::{Lan, NetConfig, data::SyncData},
    net::{Event, LocalInventory, Session},
};
use sunrise_core::nbt::Tag;
use sunrise_protocol::packets::{Empty, configuration as cfg, play};
use sunrise_world::item_defaults::ItemData;

fn connect(port: u16, name: &str, data: &Arc<ItemData>, local_data_maps: LocalDataMaps) -> Session {
    loopback::connect(
        port,
        name,
        data,
        LocalInventory {
            data_maps: local_data_maps,
            ..Default::default()
        },
    )
}

fn drive_join(host: &mut Host, session: &Session) -> Result<Vec<play::Clientbound>, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut pending = Vec::new();
    loop {
        host.tick();
        let mut joined = false;
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
                Event::Joined { .. } => joined = true,
                Event::Disconnected { reason, .. } => return Err(reason),
                Event::Packet(packet) => pending.push(*packet),
                _ => {}
            }
        }
        if joined {
            return Ok(pending);
        }
        assert!(Instant::now() < deadline, "timed out waiting for join");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn take_data_map_sync(
    pending: &mut Vec<play::Clientbound>,
    expected_registry: &str,
) -> Option<HashMap<String, Vec<(String, Tag)>>> {
    let found = pending.iter().enumerate().find_map(|(i, packet)| {
        let play::Clientbound::CustomPayload(p) = packet else {
            return None;
        };
        if p.channel != data_map_net::SYNC_CHANNEL {
            return None;
        }
        let (registry, maps) = data_map_net::decode_sync_payload(&p.data).unwrap();
        (registry == expected_registry).then_some((i, maps))
    });
    let (i, maps) = found?;
    pending.remove(i);
    Some(maps)
}

fn wait_data_map_sync(
    host: &mut Host,
    session: &Session,
    pending: &mut Vec<play::Clientbound>,
    expected_registry: &str,
) -> HashMap<String, Vec<(String, Tag)>> {
    if let Some(maps) = take_data_map_sync(pending, expected_registry) {
        return maps;
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        host.tick();
        for receipt in session.poll() {
            let (event, _credit) = receipt.into_parts();
            match event {
                Event::Disconnected { reason, .. } => {
                    panic!(
                        "disconnected while waiting for sunrise:data_map_sync for {expected_registry}: {reason}"
                    );
                }
                Event::Packet(packet) => pending.push(*packet),
                _ => {}
            }
        }
        if let Some(maps) = take_data_map_sync(pending, expected_registry) {
            return maps;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for sunrise:data_map_sync for {expected_registry}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn known(registry: &str, map_id: &str, mandatory: bool) -> KnownEntry {
    KnownEntry {
        registry: registry.to_owned(),
        map_id: map_id.to_owned(),
        mandatory,
    }
}

/// Optional and Required maps sync on join and update on reload (removing dropped entries).
#[test]
#[ignore = "daily"]
fn optional_and_required_maps_sync_on_join_and_update_on_reload() {
    let _publication = data_map_net::isolate_server_publication();
    let entries = LocalDataMaps::from_entries(vec![
        known("minecraft:item", "chemlib:fuel", true),
        known("minecraft:item", "chemlib:hint", false),
    ]);

    let mut initial = BTreeMap::new();
    initial.insert(
        "minecraft:item".to_owned(),
        vec![
            (
                "chemlib:fuel".to_owned(),
                vec![
                    ("minecraft:coal".to_owned(), Tag::Int(1600)),
                    ("minecraft:stick".to_owned(), Tag::Int(100)),
                ],
            ),
            (
                "chemlib:hint".to_owned(),
                vec![("minecraft:coal".to_owned(), Tag::Int(1))],
            ),
        ],
    );
    data_map_net::serve(Some(Arc::new(SyncedDataMapsSnapshot {
        registries: initial,
    })));

    let (_resources, data) = Resources::new("data-map-sync", "join-and-reload");
    let mut host = Host::start(SyncData::empty(), "Data map sync", entries.clone());
    let session = connect(host.port(), "SyncClient", &data, entries);
    let mut pending = drive_join(&mut host, &session).expect("matching Sunrise client must join");

    // Wait for initial play-phase sync payload.
    let synced = wait_data_map_sync(&mut host, &session, &mut pending, "minecraft:item");
    assert_eq!(
        synced["chemlib:fuel"],
        vec![
            ("minecraft:coal".to_owned(), Tag::Int(1600)),
            ("minecraft:stick".to_owned(), Tag::Int(100)),
        ]
    );
    assert_eq!(
        synced["chemlib:hint"],
        vec![("minecraft:coal".to_owned(), Tag::Int(1))]
    );

    // Simulate /reload: update coal and remove stick completely.
    let mut reloaded = BTreeMap::new();
    reloaded.insert(
        "minecraft:item".to_owned(),
        vec![
            (
                "chemlib:fuel".to_owned(),
                vec![("minecraft:coal".to_owned(), Tag::Int(2400))],
            ),
            ("chemlib:hint".to_owned(), vec![]),
        ],
    );
    data_map_net::serve(Some(Arc::new(SyncedDataMapsSnapshot {
        registries: reloaded,
    })));
    for guest in host.hub.online_players() {
        data_map_net::sync_connection(&guest.sender, &host.hub.lan.shared.data_maps_known);
    }

    let updated = wait_data_map_sync(&mut host, &session, &mut pending, "minecraft:item");
    assert_eq!(
        updated["chemlib:fuel"],
        vec![("minecraft:coal".to_owned(), Tag::Int(2400))]
    );
    assert!(updated["chemlib:hint"].is_empty());
}

/// A join whose configuration completed before `/reload` receives the reloaded generation's
/// `UpdateTags` before its `sunrise:data_map_sync` payload on the game thread.
#[test]
#[ignore = "daily"]
fn join_spanning_reload_resends_current_tags_before_data_maps() {
    use sunrise_protocol::packets::configuration::{RegistryTags, TagEntry};

    let _publication = data_map_net::isolate_server_publication();
    let entries = LocalDataMaps::from_entries(vec![known("minecraft:item", "chemlib:fuel", false)]);

    let mut initial_sync = SyncData::empty();
    initial_sync.tags.registries.push(RegistryTags {
        registry: "minecraft:item".into(),
        tags: vec![TagEntry {
            name: "test:fuels".into(),
            ids: vec![1],
        }],
    });
    let mut initial_maps = BTreeMap::new();
    initial_maps.insert(
        "minecraft:item".to_owned(),
        vec![(
            "chemlib:fuel".to_owned(),
            vec![("minecraft:coal".to_owned(), Tag::Int(100))],
        )],
    );
    data_map_net::serve(Some(Arc::new(SyncedDataMapsSnapshot {
        registries: initial_maps,
    })));

    let (_resources, data) = Resources::new("data-map-sync", "interleaved-join");
    let mut host = Host::start(initial_sync, "Data map sync", entries.clone());
    let session = connect(host.port(), "InterleavedJoin", &data, entries);

    // Finish configuration under generation 1 before Hub::tick runs begin_player_join.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut joined = false;
    while !joined {
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
                Event::Joined { .. } => joined = true,
                Event::Disconnected { reason, .. } => {
                    panic!("disconnected during config: {reason}")
                }
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for config");
        std::thread::sleep(Duration::from_millis(10));
    }

    // Commit generation 2 (new tag membership + new data map) before placement runs.
    let mut reloaded_sync = SyncData::empty();
    reloaded_sync.tags.registries.push(RegistryTags {
        registry: "minecraft:item".into(),
        tags: vec![TagEntry {
            name: "test:fuels".into(),
            ids: vec![1, 2],
        }],
    });
    host.hub.sync = Arc::new(reloaded_sync);
    let mut reloaded_maps = BTreeMap::new();
    reloaded_maps.insert(
        "minecraft:item".to_owned(),
        vec![(
            "chemlib:fuel".to_owned(),
            vec![
                ("minecraft:coal".to_owned(), Tag::Int(100)),
                ("minecraft:stick".to_owned(), Tag::Int(200)),
            ],
        )],
    );
    data_map_net::serve(Some(Arc::new(SyncedDataMapsSnapshot {
        registries: reloaded_maps,
    })));

    // Now drive placement and record play packets in arrival order.
    let mut saw_reloaded_tags = false;
    let mut saw_maps_after_tags = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !saw_maps_after_tags {
        host.tick();
        for receipt in session.poll() {
            let (event, _credit) = receipt.into_parts();
            match event {
                Event::Disconnected { reason, .. } => panic!("disconnected in play: {reason}"),
                Event::Packet(packet) => match *packet {
                    play::Clientbound::UpdateTags(tags) => {
                        if tags.registries.iter().any(|reg| {
                            reg.registry == "minecraft:item"
                                && reg
                                    .tags
                                    .iter()
                                    .any(|t| t.name == "test:fuels" && t.ids == vec![1, 2])
                        }) {
                            saw_reloaded_tags = true;
                        }
                    }
                    play::Clientbound::CustomPayload(p)
                        if p.channel == data_map_net::SYNC_CHANNEL =>
                    {
                        let (registry, maps) = data_map_net::decode_sync_payload(&p.data).unwrap();
                        if registry == "minecraft:item" {
                            assert!(
                                saw_reloaded_tags,
                                "reloaded UpdateTags must arrive before sunrise:data_map_sync"
                            );
                            assert_eq!(
                                maps["chemlib:fuel"],
                                vec![
                                    ("minecraft:coal".to_owned(), Tag::Int(100)),
                                    ("minecraft:stick".to_owned(), Tag::Int(200)),
                                ]
                            );
                            saw_maps_after_tags = true;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for paired UpdateTags + sunrise:data_map_sync"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Client missing a `DataMapSync::Required` map disconnects naming the map
/// (`ClientRegistryManager.handleKnownDataMaps`).
#[test]
#[ignore = "daily"]
fn required_map_missing_on_client_disconnects_naming_map() {
    let _publication = data_map_net::isolate_server_publication();
    let server_known =
        LocalDataMaps::from_entries(vec![known("minecraft:item", "chemlib:fuel", true)]);
    let client_known = LocalDataMaps::from_entries(vec![]);

    let (_resources, data) = Resources::new("data-map-sync", "missing-required");
    let mut host = Host::start(SyncData::empty(), "Data map sync", server_known);
    let session = connect(host.port(), "MissingRequired", &data, client_known);
    let reason = drive_join(&mut host, &session).unwrap_err();
    assert!(
        reason.contains("chemlib:fuel") && reason.contains("minecraft:item"),
        "expected disconnect naming chemlib:fuel (minecraft:item), got: {reason}"
    );
}

/// Sunrise client requiring a `DataMapSync::Required` map absent on an optional-only server
/// disconnects naming the map (`ClientRegistryManager.handleKnownDataMaps`).
#[test]
#[ignore = "daily"]
fn client_required_map_missing_on_optional_server_disconnects_naming_map() {
    let _publication = data_map_net::isolate_server_publication();
    let server_known =
        LocalDataMaps::from_entries(vec![known("minecraft:item", "chemlib:opt_hint", false)]);
    let client_known =
        LocalDataMaps::from_entries(vec![known("minecraft:item", "boiler:heat", true)]);

    let (_resources, data) = Resources::new("data-map-sync", "client-required-missing-on-server");
    let mut host = Host::start(SyncData::empty(), "Data map sync", server_known);
    let session = connect(host.port(), "ClientNeedsHeat", &data, client_known);
    let reason = drive_join(&mut host, &session).unwrap_err();
    assert!(
        reason.contains("boiler:heat") && reason.contains("minecraft:item"),
        "expected disconnect naming boiler:heat (minecraft:item), got: {reason}"
    );
}

/// Vanilla client joining a server with a `DataMapSync::Required` map is disconnected
/// with a plain message naming the map (`RegistryDataMapNegotiation.run:37-50`).
#[test]
#[ignore = "daily"]
fn required_map_disconnects_vanilla_client() {
    let _publication = data_map_net::isolate_server_publication();
    let server_known =
        LocalDataMaps::from_entries(vec![known("minecraft:item", "chemlib:fuel", true)]);

    let net = NetConfig {
        announce: false,
        ..NetConfig::loopback(0, "Vanilla required data map".into(), 8)
    };
    let lan = Lan::start_with_data_maps(Arc::new(SyncData::empty()), net, server_known).unwrap();
    let mut c = connect_vanilla_to_config(lan.port, "Vanilla");
    let reason = loop {
        let (id, payload) = c.recv().unwrap();
        match cfg::Clientbound::decode(id, &payload).unwrap() {
            cfg::Clientbound::SelectKnownPacks(_) => c
                .send(
                    &cfg::Serverbound::SelectKnownPacks(cfg::SelectKnownPacks { packs: vec![] })
                        .encode(),
                )
                .unwrap(),
            cfg::Clientbound::Disconnect(d) => {
                break sunrise_protocol::text::plain(&d.reason, &Default::default());
            }
            cfg::Clientbound::FinishConfiguration(_) => {
                panic!("vanilla client must be disconnected when server has mandatory data maps");
            }
            _ => {}
        }
    };
    assert!(
        reason.contains("chemlib:fuel") && reason.contains("minecraft:item"),
        "expected disconnect naming chemlib:fuel (minecraft:item), got: {reason}"
    );
}

/// Optional-only server with a published optional row allows both a vanilla client and a Sunrise
/// client without the map to enter play without receiving `sunrise:data_map_sync`.
#[test]
#[ignore = "daily"]
fn optional_only_server_allows_vanilla_client_and_sunrise_client_without_map() {
    let _publication = data_map_net::isolate_server_publication();
    let server_known =
        LocalDataMaps::from_entries(vec![known("minecraft:item", "chemlib:opt_hint", false)]);
    let client_known = LocalDataMaps::from_entries(vec![]);

    let mut published = BTreeMap::new();
    published.insert(
        "minecraft:item".to_owned(),
        vec![(
            "chemlib:opt_hint".to_owned(),
            vec![("minecraft:coal".to_owned(), Tag::Int(42))],
        )],
    );
    data_map_net::serve(Some(Arc::new(SyncedDataMapsSnapshot {
        registries: published,
    })));

    // 1. Sunrise client without the optional map joins cleanly into play and receives no sync.
    let (_resources, data) = Resources::new("data-map-sync", "optional-only");
    let mut host = Host::start(SyncData::empty(), "Data map sync", server_known);
    let session = connect(host.port(), "NoModSunrise", &data, client_known);
    let pending =
        drive_join(&mut host, &session).expect("Sunrise client without optional map must join");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_commands = false;
    for packet in pending {
        match packet {
            play::Clientbound::CustomPayload(p) if p.channel == data_map_net::SYNC_CHANNEL => {
                panic!(
                    "Sunrise client without optional map must not receive sunrise:data_map_sync"
                );
            }
            play::Clientbound::Commands(_) => saw_commands = true,
            _ => {}
        }
    }
    while !saw_commands {
        host.tick();
        for receipt in session.poll() {
            let (event, _credit) = receipt.into_parts();
            match event {
                Event::Disconnected { reason, .. } => {
                    panic!("Sunrise client without optional map disconnected in play: {reason}");
                }
                Event::Packet(packet) => match *packet {
                    play::Clientbound::CustomPayload(p)
                        if p.channel == data_map_net::SYNC_CHANNEL =>
                    {
                        panic!(
                            "Sunrise client without optional map must not receive sunrise:data_map_sync"
                        );
                    }
                    play::Clientbound::Commands(_) => saw_commands = true,
                    _ => {}
                },
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for play Commands on Sunrise client"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // 2. Vanilla client joins cleanly into play without receiving sunrise:data_map_known or sync.
    let mut c = connect_vanilla_to_config(host.port(), "VanillaOpt");
    loop {
        let (id, payload) = c.recv().unwrap();
        match cfg::Clientbound::decode(id, &payload).unwrap() {
            cfg::Clientbound::SelectKnownPacks(_) => c
                .send(
                    &cfg::Serverbound::SelectKnownPacks(cfg::SelectKnownPacks { packs: vec![] })
                        .encode(),
                )
                .unwrap(),
            cfg::Clientbound::CustomPayload(p) if p.channel == data_map_net::KNOWN_CHANNEL => {
                panic!("vanilla client must not receive sunrise:data_map_known");
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
    c.reader
        .set_read_timeout(Some(Duration::from_millis(25)))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_login = false;
    loop {
        host.tick();
        match c.recv() {
            Ok((id, payload)) => match play::Clientbound::decode(id, &payload).unwrap() {
                play::Clientbound::Login(_) => saw_login = true,
                play::Clientbound::CustomPayload(p) if p.channel == data_map_net::SYNC_CHANNEL => {
                    panic!("vanilla client must not receive sunrise:data_map_sync");
                }
                play::Clientbound::Commands(_) => break,
                _ => {}
            },
            Err(err) if err.is_timed_out() => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for vanilla play Commands"
                );
            }
            Err(err) => panic!("unexpected play recv error for vanilla client: {err}"),
        }
    }
    assert!(saw_login, "vanilla client must receive play Login");
}
