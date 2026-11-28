//! Shared loopback `Lan` + `Hub` + `net::Session` driver for configuration/play sync tests.
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use sunrise_client::{
    chunk::MIN_Y,
    data_map_net::LocalDataMaps,
    lan::{
        Lan, NetConfig,
        chunks::ChunkEncoder,
        data::SyncData,
        hub::{HostEvent, Hub, HubConfig, TickCtx},
    },
    net::{LocalInventory, Session},
    world::World,
};
use sunrise_protocol::{
    PROTOCOL_VERSION,
    connection::Connection,
    crypto::offline_uuid,
    packets::{
        Empty,
        configuration::{self as cfg, ClientInformation},
        handshake, login,
    },
};
use sunrise_world::{
    item_defaults::ItemData,
    native_load_context::{DataLoadOrigin, LoadContext, LoadInputs},
    registry::BlockStateId,
};

fn load_context(origin: DataLoadOrigin) -> LoadContext {
    LoadContext::new(
        origin,
        Arc::new(LoadInputs::new(BTreeMap::new(), Default::default())),
        [],
    )
}

pub struct Resources(std::path::PathBuf);

impl Resources {
    pub fn new(prefix: &str, name: &str) -> (Self, Arc<ItemData>) {
        let root =
            std::env::temp_dir().join(format!("sunrise-{prefix}-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("items.json"), "{}").unwrap();
        let data = ItemData::load(&root, load_context(DataLoadOrigin::ClientPreload)).unwrap();
        (Self(root), Arc::new(data))
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

pub struct Host {
    pub hub: Hub,
    world: World,
    next_entity: u32,
}

impl Host {
    pub fn start(sync: SyncData, title: &str, server_known: LocalDataMaps) -> Self {
        let sync = Arc::new(sync);
        let net = NetConfig {
            announce: false,
            ..NetConfig::loopback(0, title.into(), 8)
        };
        let lan = Lan::start_with_data_maps(sync.clone(), net, server_known).unwrap();
        let encoder = ChunkEncoder::new(
            [] as [(Arc<str>, bool); 0],
            &sunrise_protocol::registry::Builtin::default(),
            &[],
            &[],
            Default::default(),
        );
        let config = HubConfig {
            allow_flight: false,
            default_game_mode: 0,
            guest_commands: false,
            force_game_mode: false,
            hardcore: false,
            view_distance: 3,
            simulation_distance: 3,
            integrated: true,
            dimension: "minecraft:overworld".into(),
            seed: 1,
            max_players: 8,
            levels: Default::default(),
            level_seeds: Default::default(),
        };
        let load = Arc::new(
            load_context(DataLoadOrigin::ServerDataLoad)
                .loop_context(sunrise_world::native_load_context::LoopSide::Server, []),
        );
        Self {
            hub: Hub::new(lan, sync, encoder, config, load),
            world: World::new(BlockStateId(0), 0),
            next_entity: 100,
        }
    }

    pub fn port(&self) -> u16 {
        self.hub.lan.port
    }

    pub fn tick(&mut self) {
        let mut ctx = TickCtx {
            sea_level: 63,
            world: &mut self.world,
            entities: &[],
            next_entity: &mut self.next_entity,
            spawn: [8.5, f64::from(MIN_Y + 5), 8.5],
            spawn_angle: [0.0; 2],
            spawn_dimension: "minecraft:overworld",
            difficulty: 2,
            difficulty_locked: false,
            clock_rate: 1.0,
            tick_rate: 20.0,
            tick_frozen: false,
            tick_steps: Default::default(),
            collision: None,
            border: &sunrise_client::worldborder::WorldBorder::default(),
            streaming: None,
        };
        for event in self.hub.tick(&mut ctx) {
            if let HostEvent::Joined(conn, _) = event
                && let Some(i) = self.hub.begin_player_join(conn, &ctx)
            {
                self.hub
                    .send_commands(i, self.hub.commands_packet(self.hub.guests[i].permission));
                self.hub.finish_player_join(i, &ctx);
            }
        }
    }
}

pub fn connect(port: u16, name: &str, data: &Arc<ItemData>, inventory: LocalInventory) -> Session {
    Session::connect(
        &format!("127.0.0.1:{port}"),
        name,
        None,
        ClientInformation::new(8),
        Arc::new(Default::default()),
        Arc::clone(data),
        Arc::default(),
        inventory,
    )
}

/// Complete handshake, login, and initial vanilla configuration (`ClientInformation` +
/// `minecraft:brand = "vanilla"`), returning the connection ready to read configuration packets.
pub fn connect_vanilla_to_config(port: u16, name: &str) -> Connection {
    let mut c = Connection::connect("127.0.0.1", port, Duration::from_secs(5)).unwrap();
    c.reader
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    c.send(
        &handshake::Serverbound::Intention(handshake::Intention {
            protocol: PROTOCOL_VERSION,
            host: "127.0.0.1".into(),
            port,
            intent: handshake::LOGIN,
        })
        .encode(),
    )
    .unwrap();
    c.send(
        &login::Serverbound::Hello(login::ServerHello {
            name: name.into(),
            uuid: offline_uuid(name),
        })
        .encode(),
    )
    .unwrap();
    loop {
        let (id, payload) = c.recv().unwrap();
        match login::Clientbound::decode(id, &payload).unwrap() {
            login::Clientbound::Compression(p) => c.set_compression(p.threshold),
            login::Clientbound::LoginFinished(_) => {
                c.send(&login::Serverbound::LoginAcknowledged(Empty).encode())
                    .unwrap();
                break;
            }
            other => panic!("unexpected login packet {other:?}"),
        }
    }
    c.send(&cfg::Serverbound::ClientInformation(ClientInformation::new(4)).encode())
        .unwrap();
    let mut brand = ::sunrise_core::vanilla_bytes::Writer::new();
    brand.string("vanilla");
    c.send(
        &cfg::Serverbound::CustomPayload(cfg::CustomPayload {
            channel: "minecraft:brand".into(),
            data: brand.data,
        })
        .encode(),
    )
    .unwrap();
    c
}
