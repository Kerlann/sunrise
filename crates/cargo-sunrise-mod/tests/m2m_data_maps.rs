//! Two separately built native mods prove data maps (one `DataMap` trait for built-in keys
//! and custom-registry `EntryKey<T>`, mergers, partial removers, and `Registrar` load
//! conditions) end-to-end (#1097, #1199):
//!
//! 1. `chemlib` (library mod `1.0.0`):
//!    - defines `pub struct Gas`, `pub const GASES: RegistryKey<Gas>`, and
//!      `impl CustomRegistryEntry for Gas`;
//!    - defines and registers two maps through the same `DataMap` trait and
//!      `r.register_data_map(...)`:
//!      - `FuelMap` (`type Entry = ItemKey`, id `"chemlib:fuel"`, `DataMapSync::Required`,
//!        custom `merge` + `remove_partial`);
//!      - `GasHeatMap` (`type Entry = EntryKey<Gas>`, id `"chemlib:gas_heat"`,
//!        `DataMapSync::Optional`);
//!    - registers custom load condition `chemlib:has_gas` via `r.register_load_condition`;
//!    - ships base `data/chemlib/data_maps/item/fuel.json`.
//! 2. `boiler` (dependent mod):
//!    - registers `boiler:steam` in `chemlib::GASES`;
//!    - ships `data/chemlib/data_maps/item/fuel.json` (merging into and partially removing
//!      `chemlib`'s entries) and `data/chemlib/data_maps/chemlib/gas/gas_heat.json`
//!      (attaching heat data to `boiler:steam`, filtered by `chemlib:has_gas`);
//!    - verifies `coal_key.data(host, &chemlib::FuelMap)`, `stick_key.data(host, &chemlib::FuelMap)`,
//!      `steam_key.data(host, &chemlib::GasHeatMap)`, `oxygen_key.data(host, &chemlib::GasHeatMap)`,
//!      and `ReloadContext::data_map(&steam_key, &chemlib::GasHeatMap)` in `data_maps_updated`.

use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use sunrise_mod_package::Package;

#[path = "support/own_sdk.rs"]
mod own_sdk;

use own_sdk::{passed, server_profile, write_file};

const CHEMLIB_SOURCE: &str = r#"
use mod_api::{
    ConditionError, CustomRegistryEntry, DataMap, DataMapSync, EntryKey, Hooks, Identifier,
    ItemKey, LoadCondition, Registrar, RegistrationContext, RegistryKey, RegistryOptions, Tag,
};
use std::sync::OnceLock;

#[derive(Debug, PartialEq, Eq)]
pub struct Gas {
    pub color: u32,
    pub toxic: bool,
}

pub const GASES: RegistryKey<Gas> = RegistryKey::new("chemlib:gas");

impl CustomRegistryEntry for Gas {
    const REGISTRY: RegistryKey<Self> = GASES;
}

static OXYGEN: OnceLock<EntryKey<Gas>> = OnceLock::new();

pub fn oxygen_key() -> EntryKey<Gas> {
    *OXYGEN.get().expect("chemlib registered oxygen")
}

pub struct FuelMap;

impl DataMap for FuelMap {
    type Entry = ItemKey;
    type Value = Vec<i32>;

    fn id(&self) -> &str {
        "chemlib:fuel"
    }

    fn decode(&self, value: &Tag) -> Option<Vec<i32>> {
        let Tag::List(items) = value else {
            return None;
        };
        items
            .iter()
            .map(|item| match item {
                Tag::Int(n) => Some(*n),
                _ => None,
            })
            .collect()
    }

    fn sync(&self) -> DataMapSync {
        DataMapSync::Required
    }

    fn merge(&self, _entry: &Identifier, existing: &Tag, incoming: &Tag) -> Result<Tag, String> {
        match (existing, incoming) {
            (Tag::List(a), Tag::List(b)) => {
                let mut merged = a.clone();
                merged.extend(b.iter().cloned());
                Ok(Tag::List(merged))
            }
            _ => Err("expected list values for chemlib:fuel".into()),
        }
    }

    fn remove_partial(
        &self,
        _entry: &Identifier,
        existing: &Tag,
        remover: &Tag,
    ) -> Result<Option<Tag>, String> {
        match (existing, remover) {
            (Tag::List(items), Tag::Int(drop_val)) => {
                let kept: Vec<Tag> = items
                    .iter()
                    .filter(|item| !matches!(item, Tag::Int(v) if v == drop_val))
                    .cloned()
                    .collect();
                if kept.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Tag::List(kept)))
                }
            }
            _ => Err("expected list value and int remover for chemlib:fuel".into()),
        }
    }
}

pub struct GasHeatMap;

impl DataMap for GasHeatMap {
    type Entry = EntryKey<Gas>;
    type Value = i32;

    fn id(&self) -> &str {
        "chemlib:gas_heat"
    }

    fn decode(&self, value: &Tag) -> Option<i32> {
        match value {
            Tag::Int(n) => Some(*n),
            _ => None,
        }
    }

    fn sync(&self) -> DataMapSync {
        DataMapSync::Optional
    }
}

pub struct SecretMap;

impl DataMap for SecretMap {
    type Entry = ItemKey;
    type Value = i32;

    fn id(&self) -> &str {
        "chemlib:secret"
    }

    fn decode(&self, value: &Tag) -> Option<i32> {
        match value {
            Tag::Int(n) => Some(*n),
            _ => None,
        }
    }
}

struct HasGas;

impl LoadCondition for HasGas {
    fn test(&self, args: &mod_api::serde_json::Value) -> Result<bool, ConditionError> {
        let gas = args
            .get("gas")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConditionError::new("expected string field `gas`"))?;
        Ok(gas == "boiler:steam")
    }
}

#[derive(Default)]
struct ChemLib;

#[mod_api::sunrise_mod]
impl Hooks for ChemLib {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        r.declare_registry(GASES, RegistryOptions::new().sync(true));
        let oxygen = r.register_entry(
            GASES,
            "chemlib:oxygen",
            Gas {
                color: 0xAA_DD_FF,
                toxic: false,
            },
        );
        let _ = OXYGEN.set(oxygen);
        r.register_data_map(FuelMap);
        r.register_data_map(GasHeatMap);
        r.register_data_map(SecretMap);
        r.register_load_condition("chemlib:has_gas", HasGas);
    }
}
"#;

const BOILER_SOURCE: &str = r#"
use mod_api::{
    DataMapsUpdateCause, DataMapsUpdatedEvent, Hooks, Identifier, Registrar, RegistrationContext,
    ReloadContext, ServerHost, ServerStartedEvent,
};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

static STEAM: OnceLock<mod_api::EntryKey<chemlib::Gas>> = OnceLock::new();
static RELOAD_GAS_OK: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Boiler;

#[mod_api::sunrise_mod]
impl Hooks for Boiler {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        let steam = r.register_entry(
            chemlib::GASES,
            "boiler:steam",
            chemlib::Gas {
                color: 0xEE_EE_EE,
                toxic: false,
            },
        );
        let _ = STEAM.set(steam);
    }

    fn data_maps_updated(
        &self,
        context: &dyn ReloadContext,
        event: &mut DataMapsUpdatedEvent<'_>,
    ) {
        if event.registry == "chemlib:gas" && event.cause == DataMapsUpdateCause::ServerReload {
            let steam_key = *STEAM.get().expect("boiler:steam registered");
            if context.data_map(&steam_key, &chemlib::GasHeatMap) == Ok(Some(373)) {
                RELOAD_GAS_OK.store(true, Ordering::SeqCst);
            }
        }
    }

    fn server_started(&self, host: &mut dyn ServerHost, _: &mut ServerStartedEvent) {
        let coal_key = host
            .item_key(&Identifier::parse("minecraft:coal").unwrap())
            .expect("minecraft:coal key");
        let stick_key = host
            .item_key(&Identifier::parse("minecraft:stick").unwrap())
            .expect("minecraft:stick key");
        let steam_key = *STEAM.get().expect("boiler:steam registered");
        let oxygen_key = chemlib::oxygen_key();

        let item_ok = coal_key.data(host, &chemlib::FuelMap) == Ok(Some(vec![200, 300]))
            && stick_key.data(host, &chemlib::FuelMap) == Ok(None);
        let secret_ok = coal_key.data(host, &chemlib::SecretMap) == Ok(Some(999));
        let gas_ok = steam_key.data(host, &chemlib::GasHeatMap) == Ok(Some(373));
        let cond_ok = oxygen_key.data(host, &chemlib::GasHeatMap) == Ok(None);
        let reload_ok = RELOAD_GAS_OK.load(Ordering::SeqCst);

        println!("m2m_data_maps item_ok={item_ok} secret_ok={secret_ok} gas_ok={gas_ok} reload_ok={reload_ok} cond_ok={cond_ok}");
    }
}
"#;

const PROTOCOL_VERSION: i32 = 777;

fn write_varint(out: &mut Vec<u8>, mut value: i32) {
    let mut v = value as u32;
    loop {
        if v & !0x7F == 0 {
            out.push(v as u8);
            break;
        }
        out.push(((v & 0x7F) as u8) | 0x80);
        v >>= 7;
    }
    let _ = &mut value;
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    write_varint(out, s.len() as i32);
    out.extend_from_slice(s.as_bytes());
}

fn read_varint_slice(buf: &[u8], pos: &mut usize) -> i32 {
    let mut result = 0u32;
    let mut shift = 0;
    loop {
        let byte = buf[*pos];
        *pos += 1;
        result |= u32::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return result as i32;
        }
        shift += 7;
        assert!(shift < 35, "varint too long");
    }
}

fn read_string_slice(buf: &[u8], pos: &mut usize) -> String {
    let len = read_varint_slice(buf, pos) as usize;
    let s = std::str::from_utf8(&buf[*pos..*pos + len])
        .unwrap()
        .to_owned();
    *pos += len;
    s
}

fn read_varint_stream(stream: &mut TcpStream) -> std::io::Result<i32> {
    let mut result = 0u32;
    let mut shift = 0;
    loop {
        let mut b = [0u8; 1];
        stream.read_exact(&mut b)?;
        result |= u32::from(b[0] & 0x7F) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(result as i32);
        }
        shift += 7;
    }
}

fn send_packet(stream: &mut TcpStream, id: i32, payload: &[u8]) {
    let mut body = Vec::with_capacity(5 + payload.len());
    write_varint(&mut body, id);
    body.extend_from_slice(payload);
    let mut frame = Vec::with_capacity(5 + body.len());
    write_varint(&mut frame, body.len() as i32);
    frame.extend_from_slice(&body);
    stream.write_all(&frame).unwrap();
    stream.flush().unwrap();
}

fn recv_packet(stream: &mut TcpStream) -> (i32, Vec<u8>) {
    let len = read_varint_stream(stream).unwrap() as usize;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).unwrap();
    let mut pos = 0;
    let id = read_varint_slice(&body, &mut pos);
    (id, body[pos..].to_vec())
}

#[derive(Debug, PartialEq, Eq)]
enum SimpleTag {
    Int(i32),
    IntList(Vec<i32>),
}

fn decode_simple_network_nbt(buf: &[u8], pos: &mut usize) -> SimpleTag {
    let tag_type = buf[*pos];
    *pos += 1;
    match tag_type {
        3 => {
            let v = i32::from_be_bytes(buf[*pos..*pos + 4].try_into().unwrap());
            *pos += 4;
            SimpleTag::Int(v)
        }
        9 => {
            let elem_type = buf[*pos];
            *pos += 1;
            let count = i32::from_be_bytes(buf[*pos..*pos + 4].try_into().unwrap()) as usize;
            *pos += 4;
            assert_eq!(elem_type, 3, "expected TAG_Int list");
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                let v = i32::from_be_bytes(buf[*pos..*pos + 4].try_into().unwrap());
                *pos += 4;
                items.push(v);
            }
            SimpleTag::IntList(items)
        }
        other => panic!("unexpected NBT tag type {other} in m2m_data_maps sync payload"),
    }
}

fn decode_sync_payload(data: &[u8]) -> (String, BTreeMap<String, Vec<(String, SimpleTag)>>) {
    let mut pos = 0;
    let registry = read_string_slice(data, &mut pos);
    let map_count = read_varint_slice(data, &mut pos) as usize;
    let mut maps = BTreeMap::new();
    for _ in 0..map_count {
        let map_id = read_string_slice(data, &mut pos);
        let entry_count = read_varint_slice(data, &mut pos) as usize;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let entry_id = read_string_slice(data, &mut pos);
            let tag = decode_simple_network_nbt(data, &mut pos);
            entries.push((entry_id, tag));
        }
        maps.insert(map_id, entries);
    }
    assert_eq!(pos, data.len(), "trailing bytes in sunrise:data_map_sync");
    (registry, maps)
}

fn wait_for_sync_registries(
    stream: &mut TcpStream,
    expected: &[&str],
) -> BTreeMap<String, BTreeMap<String, Vec<(String, SimpleTag)>>> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut collected = BTreeMap::new();
    while expected.iter().any(|reg| !collected.contains_key(*reg)) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for sunrise:data_map_sync ({collected:?})"
        );
        let (id, payload) = recv_packet(stream);
        match id {
            24 => {
                // Play Clientbound::CustomPayload
                let mut pos = 0;
                let channel = read_string_slice(&payload, &mut pos);
                if channel == "sunrise:data_map_sync" {
                    let (registry, maps) = decode_sync_payload(&payload[pos..]);
                    if expected.contains(&registry.as_str()) {
                        collected.insert(registry, maps);
                    }
                }
            }
            45 => {
                // Play Clientbound::KeepAlive -> echo on Serverbound::KeepAlive (28)
                send_packet(stream, 28, &payload);
            }
            32 => panic!("server disconnected client during data map sync"),
            _ => {}
        }
    }
    collected
}

#[test]
#[ignore = "daily"]
fn two_separately_built_mods_share_data_maps_mergers_and_load_conditions() {
    let profile = server_profile();
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(target).unwrap();
    let scratch = tempfile::tempdir_in(target).unwrap();
    let root = scratch.path().canonicalize().unwrap();

    let stub_resources = root.join("stub-resources");
    fs::create_dir(&stub_resources).unwrap();
    for (name, json) in [
        (
            "sunrise-resources.json",
            r#"{"schema":7,"minecraft":"26.3"}"#,
        ),
        ("sunrise-prepare.json", r#"{"output_version":9}"#),
        ("registries.json", "{}"),
    ] {
        fs::write(stub_resources.join(name), json).unwrap();
    }

    let tool = |dir: &Path, args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cargo-sunrise-mod"));
        cmd.current_dir(dir)
            .env("SUNRISE_MOD_CACHE", root.join("cache"))
            .env("SUNRISE_HOME", root.join("home"))
            .env("SUNRISE_LAUNCHER_HOME", root.join("launcher"))
            .env("SUNRISE_LAUNCHER_NO_SELF_UPDATE", "1")
            .env("SUNRISE_NO_AUDIO", "1")
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .args(args);
        passed(cmd.output().unwrap())
    };

    // 1. chemlib library mod (version 1.0.0) with base data_maps/item/fuel.json.
    tool(&root, &["new", "chemlib", "--template", "native"]);
    let chemlib = root.join("chemlib");
    fs::write(chemlib.join("src/lib.rs"), CHEMLIB_SOURCE).unwrap();
    let chemlib_manifest = fs::read_to_string(chemlib.join("sunrise-mod.toml"))
        .unwrap()
        .replace("0.1.0", "1.0.0");
    fs::write(chemlib.join("sunrise-mod.toml"), chemlib_manifest).unwrap();
    write_file(
        &chemlib.join("data/chemlib/data_maps/item/fuel.json"),
        r#"{"values":{"minecraft:coal":[100,200],"minecraft:stick":[50]}}"#,
    );
    write_file(
        &chemlib.join("data/chemlib/data_maps/item/secret.json"),
        r#"{"values":{"minecraft:coal":999}}"#,
    );
    let chemlib_smod = root.join("chemlib.smod");
    fs::write(&chemlib_smod, Package::pack_directory(&chemlib).unwrap()).unwrap();

    // 2. boiler dependent mod merging & partially removing fuel.json and attaching gas_heat.json.
    tool(&root, &["new", "boiler", "--template", "native"]);
    let boiler = root.join("boiler");
    fs::write(boiler.join("src/lib.rs"), BOILER_SOURCE).unwrap();
    let boiler_manifest = fs::read_to_string(boiler.join("sunrise-mod.toml")).unwrap();
    fs::write(
        boiler.join("sunrise-mod.toml"),
        format!("{boiler_manifest}\n[dependencies]\nchemlib = \"^1\"\n"),
    )
    .unwrap();
    write_file(
        &boiler.join("data/chemlib/data_maps/item/fuel.json"),
        r#"{"values":{"minecraft:coal":[300]},"remove":{"minecraft:coal":100,"minecraft:stick":50}}"#,
    );
    write_file(
        &boiler.join("data/chemlib/data_maps/chemlib/gas/gas_heat.json"),
        r#"{"values":{"boiler:steam":{"neoforge:conditions":[{"type":"chemlib:has_gas","gas":"boiler:steam"}],"value":373},"chemlib:oxygen":{"neoforge:conditions":[{"type":"chemlib:has_gas","gas":"other:absent"}],"value":90}}}"#,
    );

    let run = boiler.join("target/sunrise-mod/run");
    let _ = fs::remove_dir_all(&run);
    fs::create_dir_all(run.join("game")).unwrap();
    fs::write(run.join("game/eula.txt"), "eula=true\n").unwrap();
    let stdout = tool(
        &boiler,
        &[
            "run",
            "--with",
            chemlib_smod.to_str().unwrap(),
            "--resources",
            stub_resources.to_str().unwrap(),
            "--",
            "--dedicated",
            "--initSettings",
        ],
    );
    let executable = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Starting "))
        .and_then(|line| line.split(" with ").next())
        .map(PathBuf::from)
        .expect("run names the program it starts");
    let prog = executable
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // Start the real dedicated server with the two separately built mods, connect a real
    // Sunrise client over TCP, verify initial synced maps (and ServerOnly absence), then
    // trigger `/reload` with updated datapack files and verify the client receives the update.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let game_dir = root.join("runs/both");
    let reload_pack = game_dir.join("world/datapacks/m2m_reload");
    write_file(
        &reload_pack.join("pack.mcmeta"),
        r#"{"pack":{"description":"m2m data maps reload override","min_format":94,"max_format":94}}"#,
    );
    write_file(
        &game_dir.join("world/datapacks.json"),
        r#"{"enabled":["vanilla","file/m2m_reload"],"disabled":[]}"#,
    );
    fs::write(game_dir.join("eula.txt"), "eula=true\n").unwrap();
    fs::write(
        game_dir.join("server.properties"),
        format!(
            "online-mode=false\nserver-ip=127.0.0.1\nserver-port={port}\n\
             network-compression-threshold=-1\nspawn-protection=0\n\
             view-distance=2\nsimulation-distance=2\npause-when-empty-seconds=-1\n\
             level-type=minecraft\\:flat\ngenerate-structures=false\n"
        ),
    )
    .unwrap();

    let app = run.join("app");
    let mut server = Command::new(app.join(&prog))
        .current_dir(&game_dir)
        .env("LD_LIBRARY_PATH", &app)
        .env("SUNRISE_HOME", root.join("home/both"))
        .env("SUNRISE_LAUNCHER_HOME", root.join("launcher/both"))
        .env("SUNRISE_LAUNCHER_NO_SELF_UPDATE", "1")
        .env("SUNRISE_NO_AUDIO", "1")
        .env("XDG_DATA_HOME", root.join("data/both"))
        .env("XDG_CONFIG_HOME", root.join("config/both"))
        .args([
            "--game-dir",
            game_dir.to_str().unwrap(),
            "--resources",
            profile.to_str().unwrap(),
            "--dedicated",
            "--nogui",
            "--port",
            &port.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdout = server.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let reader_thread = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });

    let mut server_log = Vec::new();
    let mut marker_seen = false;
    let mut listening = false;
    let deadline = Instant::now() + Duration::from_secs(120);
    while !(marker_seen && listening) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "server startup timed out:\n{}",
            server_log.join("\n")
        );
        let line = rx
            .recv_timeout(remaining)
            .unwrap_or_else(|_| panic!("server exited early:\n{}", server_log.join("\n")));
        if line.starts_with("m2m_data_maps ") {
            assert_eq!(
                line,
                "m2m_data_maps item_ok=true secret_ok=true gas_ok=true reload_ok=true cond_ok=true",
                "two-mod server data maps check failed: {line}"
            );
            marker_seen = true;
        }
        if line.contains("Done (") {
            listening = true;
        }
        server_log.push(line);
    }

    // Connect a real Sunrise client over TCP and complete login + configuration.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut intention = Vec::new();
    write_varint(&mut intention, PROTOCOL_VERSION);
    write_string(&mut intention, "127.0.0.1");
    intention.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut intention, 2); // LOGIN
    send_packet(&mut stream, 0, &intention);

    let mut hello = Vec::new();
    write_string(&mut hello, "DataMapClient");
    hello.extend_from_slice(&0x0001_0002_0003_0004_0005_0006_0007_0008u128.to_be_bytes());
    send_packet(&mut stream, 0, &hello);

    loop {
        let (id, _payload) = recv_packet(&mut stream);
        match id {
            2 => {
                // LoginFinished -> LoginAcknowledged (3)
                send_packet(&mut stream, 3, &[]);
                break;
            }
            0 => panic!("login disconnected by server"),
            other => panic!("unexpected login packet id {other}"),
        }
    }

    // Configuration: send ClientInformation + minecraft:brand("sunrise").
    let mut client_info = Vec::new();
    write_string(&mut client_info, "en_us");
    client_info.push(2); // view_distance
    write_varint(&mut client_info, 0); // chat_visibility
    client_info.push(1); // chat_colors
    client_info.push(0x7F); // model_customisation
    write_varint(&mut client_info, 1); // main_hand
    client_info.push(0); // text_filtering
    client_info.push(1); // allows_listing
    write_varint(&mut client_info, 0); // particle_status
    send_packet(&mut stream, 0, &client_info);

    let mut brand_payload = Vec::new();
    write_string(&mut brand_payload, "minecraft:brand");
    write_string(&mut brand_payload, "sunrise");
    send_packet(&mut stream, 2, &brand_payload);

    let mut saw_known_data_maps = false;
    loop {
        let (id, payload) = recv_packet(&mut stream);
        match id {
            15 => {
                // SelectKnownPacks -> reply empty
                let mut reply = Vec::new();
                write_varint(&mut reply, 0);
                send_packet(&mut stream, 7, &reply);
            }
            4 => send_packet(&mut stream, 4, &payload), // KeepAlive
            1 => {
                // CustomPayload
                let mut pos = 0;
                let channel = read_string_slice(&payload, &mut pos);
                if channel == "sunrise:data_map_known" {
                    let count = read_varint_slice(&payload, &mut pos) as usize;
                    let mut advertised = Vec::with_capacity(count);
                    for _ in 0..count {
                        let reg = read_string_slice(&payload, &mut pos);
                        let map_id = read_string_slice(&payload, &mut pos);
                        let mandatory = payload[pos] != 0;
                        pos += 1;
                        advertised.push((reg, map_id, mandatory));
                    }
                    assert!(
                        advertised.contains(&(
                            "minecraft:item".to_owned(),
                            "chemlib:fuel".to_owned(),
                            true
                        )),
                        "expected mandatory chemlib:fuel in sunrise:data_map_known: {advertised:?}"
                    );
                    assert!(
                        advertised.contains(&(
                            "chemlib:gas".to_owned(),
                            "chemlib:gas_heat".to_owned(),
                            false
                        )),
                        "expected optional chemlib:gas_heat in sunrise:data_map_known: {advertised:?}"
                    );
                    assert!(
                        !advertised.iter().any(|(_, id, _)| id == "chemlib:secret"),
                        "ServerOnly map chemlib:secret must not be advertised: {advertised:?}"
                    );
                    saw_known_data_maps = true;

                    // Reply advertising both chemlib:fuel and chemlib:gas_heat.
                    let mut reply = Vec::new();
                    write_string(&mut reply, "sunrise:data_map_known_reply");
                    write_varint(&mut reply, 2); // 2 registries
                    write_string(&mut reply, "chemlib:gas");
                    write_varint(&mut reply, 1);
                    write_string(&mut reply, "chemlib:gas_heat");
                    write_string(&mut reply, "minecraft:item");
                    write_varint(&mut reply, 1);
                    write_string(&mut reply, "chemlib:fuel");
                    send_packet(&mut stream, 2, &reply);
                }
            }
            3 => {
                // FinishConfiguration -> acknowledge
                send_packet(&mut stream, 3, &[]);
                break;
            }
            2 => panic!("configuration disconnected by server"),
            _ => {}
        }
    }
    assert!(
        saw_known_data_maps,
        "server must send sunrise:data_map_known during configuration"
    );

    // Play phase: verify initial synced data maps from both mods.
    let initial_sync = wait_for_sync_registries(&mut stream, &["minecraft:item", "chemlib:gas"]);
    assert_eq!(
        initial_sync["minecraft:item"].get("chemlib:fuel"),
        Some(&vec![(
            "minecraft:coal".to_owned(),
            SimpleTag::IntList(vec![200, 300]),
        )])
    );
    assert!(!initial_sync["minecraft:item"].contains_key("chemlib:secret"));
    assert_eq!(
        initial_sync["chemlib:gas"].get("chemlib:gas_heat"),
        Some(&vec![("boiler:steam".to_owned(), SimpleTag::Int(373))])
    );

    // Write reload override files into the world datapack and trigger `/reload`.
    write_file(
        &reload_pack.join("data/chemlib/data_maps/item/fuel.json"),
        r#"{"replace":true,"values":{"minecraft:coal":[777]}}"#,
    );
    write_file(
        &reload_pack.join("data/chemlib/data_maps/chemlib/gas/gas_heat.json"),
        r#"{"replace":true,"values":{"boiler:steam":500}}"#,
    );
    let stdin = server.stdin.as_mut().unwrap();
    writeln!(stdin, "reload").unwrap();
    stdin.flush().unwrap();

    // Verify the client receives the updated synced data maps after `/reload`.
    let reloaded_sync = wait_for_sync_registries(&mut stream, &["minecraft:item", "chemlib:gas"]);
    assert_eq!(
        reloaded_sync["minecraft:item"].get("chemlib:fuel"),
        Some(&vec![(
            "minecraft:coal".to_owned(),
            SimpleTag::IntList(vec![777]),
        )])
    );
    assert!(!reloaded_sync["minecraft:item"].contains_key("chemlib:secret"));
    assert_eq!(
        reloaded_sync["chemlib:gas"].get("chemlib:gas_heat"),
        Some(&vec![("boiler:steam".to_owned(), SimpleTag::Int(500))])
    );

    drop(stream);
    let _ = writeln!(stdin, "stop");
    let _ = stdin.flush();
    let status = server.wait().unwrap();
    let _ = reader_thread.join();
    assert!(status.success(), "server did not stop cleanly: {status}");
}
