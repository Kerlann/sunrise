use super::*;
use crate::game::advancements::block_use_recording::{Recording, SEEN};
use crate::game::native_entity_refs::tests::{
    fixture_root, prepare_entity_ticking_neighborhood, run_with_game_stack, with_game_role,
};
use crate::game::native_guest_turn_tests::open_fixture_hub;
use mod_api::{
    BlockPart, CommandSource, Flow, Hooks, HostSide, InteractionResult, ModEntry, PermissionLevel,
    Registrar, RegistrationContext, ServerHost, TriState,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use sunrise_core::{position::ChunkPos, vanilla_bytes::Writer};
use sunrise_gameplay_common::game::{
    GameGameExt1 as _, TryIntoApi as _, end_state::END_SPAWN, execution_role::ExecutionRole,
    lan_actions::TurnRefused, server_side::server_travel::credits_respawn_tests::placed_guest,
    tables::WorldTablesExt as _,
};
use sunrise_items::items::ItemStack;
use sunrise_level::chunk::Chunk;
use sunrise_protocol::{
    buf::{Codec, Reader},
    packets::{
        configuration::CustomPayload,
        play::{self, Clientbound as C, Serverbound as S},
    },
};
use sunrise_sim::{
    lan::{PlayerSender, hub::game_event},
    raw_payloads::{Side, Transport, connection::record_overhead, negotiated},
};

const USE_CHANNEL: &str = sunrise_protocol::packets::custom_payload::CAULDRON_USE_CHANNEL;
const A: u32 = 12;
const A_UUID: u128 = 342;
const B: u32 = 13;
const B_UUID: u128 = 343;
const HIT: [i32; 3] = [3, 64, 4];
const BESIDE: [i32; 3] = [3, 65, 4];

fn add_player(game: &mut ServerGame<'_>, conn: u32) -> impl Fn() -> Vec<C> + use<> {
    let (mut guest, _) = placed_guest(conn, Dim::Overworld, [3.5, 64.0, 6.0]);
    let transport = Transport::capture(Side::Server, record_overhead(&[])).unwrap();
    transport
        .accept(transport.local.manifest.entries())
        .unwrap();
    transport.finish_configuration().unwrap();
    assert!(negotiated(&transport, USE_CHANNEL));
    let (sender, inspect) = PlayerSender::payload_executor_test_sender(transport);
    guest.sender = sender;
    game.server.next_entity = game.server.next_entity.max(conn);
    game.server.lan.as_mut().unwrap().hub.guests.push(guest);
    move || {
        inspect()
            .iter()
            .filter_map(|body| {
                let mut reader = Reader::new(body);
                C::decode(reader.varint().ok()?, reader.rest()).ok()
            })
            .collect()
    }
}

fn use_on(game: &mut ServerGame<'_>, pos: [i32; 3], sequence: i32) {
    let click = play::UseItemOn {
        hand: 0,
        pos,
        face: 1,
        cursor: [0.5, 1.0, 0.5],
        inside: false,
        world_border: false,
        sequence,
    };
    let mut body = Writer::new();
    body.i64(i64::from(sequence));
    play::UseItemOn::write(&click, &mut body);
    game.lan_guest_packet(
        A,
        S::CustomPayload(CustomPayload {
            channel: USE_CHANNEL.into(),
            data: body.data,
        }),
    );
    let player = game.server.lan.as_ref().unwrap().hub.guest(A).unwrap();
    assert_eq!(player.ack_up_to.get(), sequence);
}

fn receipt(packet: &C, sequence: i32) -> Option<[bool; 2]> {
    let C::CustomPayload(p) = packet else {
        return None;
    };
    if p.channel != USE_CHANNEL {
        return None;
    }
    let mut r = Reader::new(&p.data);
    if r.i64().ok()? != i64::from(sequence) {
        return None;
    }
    let outcome = [r.bool().unwrap(), r.bool().unwrap()];
    assert_eq!(r.remaining(), 0);
    Some(outcome)
}

fn packet_index(packets: &[C], matches: impl FnMut(&C) -> bool) -> usize {
    packets.iter().position(matches).expect("expected packet")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Travel {
    Allow,
    Refuse,
    Disconnect,
}

struct TravelListener {
    mode: Travel,
    events: Arc<Mutex<Vec<&'static str>>>,
}

#[mod_api::__subscriptions]
impl Hooks for TravelListener {
    fn entity_travel_to_dimension(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::EntityTravelToDimensionEvent<'_>,
    ) -> Flow {
        assert_eq!(event.cause, "end_portal");
        assert_eq!(event.dimension, Dim::End.id());
        assert_eq!(host.dimension(), Dim::Overworld.id());
        let player = host.player(A_UUID).unwrap();
        assert_eq!(event.entity, player.entity());
        assert_eq!(
            player.level(host).unwrap().dimension(),
            Dim::Overworld.key()
        );
        self.events.lock().unwrap().push("travel");
        match self.mode {
            Travel::Allow => Flow::Continue,
            Travel::Refuse => Flow::Cancel,
            Travel::Disconnect => {
                assert!(
                    player
                        .disconnect(host, mod_api::Text::literal("Leaving"))
                        .unwrap()
                );
                Flow::Continue
            }
        }
    }

    fn level_load(&self, host: &mut dyn ServerHost, event: &mut mod_api::LevelLoadEvent<'_>) {
        if event.dimension == Dim::End.id() {
            assert_eq!(host.dimension(), Dim::End.id());
            assert!(host.level(event.dimension).unwrap().in_place(host));
            self.events.lock().unwrap().push("load");
        }
    }

    fn portal_create(&self, host: &mut dyn ServerHost, event: &mut mod_api::PortalCreateEvent<'_>) {
        assert_eq!(event.dimension, Dim::End.id());
        let [x, y, z] = END_SPAWN;
        assert_eq!(event.pos, [x, y - 1, z]);
        let level = host.level(event.dimension).unwrap();
        let block = level.block_state(host, [x, y - 2, z]).unwrap();
        assert_eq!(block.block().id(host), Some("minecraft:obsidian"));
        assert_eq!(host.players().acting_player().unwrap().uuid(), A_UUID);
        self.events.lock().unwrap().push("platform");
    }

    fn player_changed_dimension(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::PlayerChangedDimensionEvent<'_>,
    ) {
        assert_eq!((event.from, event.to), (Dim::Overworld.id(), Dim::End.id()));
        assert_eq!(event.player, A_UUID);
        assert_eq!(host.dimension(), Dim::End.id());
        assert_eq!(host.players().acting_player().unwrap().uuid(), A_UUID);
        self.events.lock().unwrap().push("changed");
    }
}

fn spectator_arrival(role: ExecutionRole, mode: Travel) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mods = vec![ModEntry::new(
        "observe-travel",
        Box::new(TravelListener {
            mode,
            events: events.clone(),
        }),
    )]
    .into();
    mod_platform::testing::with_mods(mods, || {
        with_game_role(fixture_root(), role, |game| {
            let scratch = tempfile::tempdir().unwrap();
            game.server.world_root = scratch.path().to_path_buf();
            if role == ExecutionRole::DedicatedServer {
                game.options.dedicated = Some(Vec::new());
            }
            open_fixture_hub(game);
            let out = add_player(game, A);
            let watcher = add_player(game, B);
            game.server
                .lan
                .as_mut()
                .unwrap()
                .hub
                .update_guest(A, |guest| {
                    guest.game_mode = 3;
                })
                .unwrap();
            prepare_entity_ticking_neighborhood(game, ChunkPos::new(0, 0));
            game.set_state_named(HIT, "minecraft:end_portal", &[])
                .unwrap();
            assert!(game.server.levels.0.parked_world(Dim::End).is_none());

            use_on(game, HIT, 7);
            assert_eq!(game.dim, Dim::Overworld);
            if mode != Travel::Allow {
                assert_eq!(*events.lock().unwrap(), ["travel"]);
                assert!(!game.has_pending_travel(A));
                assert!(game.server.levels.0.parked_world(Dim::End).is_none());
                let packets = out();
                assert!(
                    !packets
                        .iter()
                        .any(|p| matches!(p, C::Respawn(_) | C::PlayerPosition(_)))
                );
                if mode == Travel::Refuse {
                    assert_eq!(
                        packets.iter().find_map(|p| receipt(p, 7)),
                        Some([false, false])
                    );
                }
                return;
            }

            assert_eq!(*events.lock().unwrap(), ["travel", "load"]);
            let guest = game.server.lan.as_ref().unwrap().hub.guest(A).unwrap();
            assert_eq!(guest.dimension, Dim::End.id());
            assert!(guest.landing && game.has_pending_travel(A));
            let before = out();
            let respawn = packet_index(&before, |p| matches!(p, C::Respawn(_)));
            let replied = packet_index(&before, |p| receipt(p, 7) == Some([false, false]));
            assert!(respawn < replied);
            assert!(!before.iter().any(|p| matches!(p, C::PlayerPosition(_))));

            game.enter_level(Dim::End).unwrap();
            let [x, y, z] = END_SPAWN;
            let (cx, cz) = (x >> 4, z >> 4);
            for dx in -2..=2 {
                for dz in -2..=2 {
                    game.server
                        .streaming
                        .insert_remote(&mut game.common.world, Chunk::empty(cx + dx, cz + dz));
                }
            }
            game.set_state_named([x, y - 2, z], "minecraft:stone", &[])
                .unwrap();
            game.set_state_named([x, y - 1, z], "minecraft:dirt", &[])
                .unwrap();
            game.tick_lan(true);
            assert_eq!(
                *events.lock().unwrap(),
                ["travel", "load", "platform", "changed"]
            );
            assert_eq!(&*game.block_name([x, y - 2, z]), "minecraft:obsidian");
            assert_eq!(&*game.block_name([x, y - 1, z]), "minecraft:air");
            assert!(
                game.server
                    .tickets
                    .0
                    .portal_tickets()
                    .iter()
                    .any(|ticket| ticket.chunk == (cx, cz))
            );
            assert!(!game.has_pending_travel(A));
            let packets = out();
            let position = packet_index(&packets, |p| matches!(p, C::PlayerPosition(_)));
            let ready = packet_index(
                &packets,
                |p| matches!(p, C::GameEvent(e) if e.event == game_event::LEVEL_CHUNKS_LOAD_START),
            );
            let ack = packet_index(
                &packets,
                |p| matches!(p, C::BlockChangedAck(p) if p.sequence == 7),
            );
            assert!(replied < ack && ack < position && position < ready);
            assert!(
                !watcher()
                    .iter()
                    .any(|p| matches!(p, C::Respawn(_) | C::PlayerPosition(_))
                        || receipt(p, 7).is_some())
            );
        });
    });
}

#[test]
#[ignore = "daily"]
fn spectator_use_packet_opens_and_lands_in_the_end() {
    run_with_game_stack(|| {
        for role in [
            ExecutionRole::IntegratedServer,
            ExecutionRole::DedicatedServer,
        ] {
            for mode in [Travel::Allow, Travel::Refuse, Travel::Disconnect] {
                spectator_arrival(role, mode);
            }
        }
    });
}

struct BlockListener {
    suppress: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<&'static str>>>,
}

struct StonePart(Arc<Mutex<Vec<&'static str>>>);

#[mod_api::sunrise_part]
impl BlockPart for StonePart {
    fn use_item_on(
        &self,
        _: HostSide<'_>,
        event: &mut mod_api::UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        assert_eq!(event.click.pos, HIT);
        assert_eq!(event.click.player, A_UUID);
        assert!(event.click.stack.is_some());
        self.0.lock().unwrap().push("block");
        None
    }
}

#[mod_api::__subscriptions]
impl Hooks for BlockListener {
    fn register(&self, registrar: &mut dyn Registrar, _: &RegistrationContext) {
        let stone = registrar.block_key("minecraft:stone");
        registrar.block_part(stone, StonePart(self.events.clone()));
    }

    fn register_commands(
        &self,
        _: &mut dyn ServerHost,
        event: &mut mod_api::RegisterCommandsEvent<'_>,
    ) {
        let events = self.events.clone();
        event
            .commands
            .push(
                mod_api::Command::new("checknested").executes(move |host, source, command| {
                    let CommandSource::Player { player, permission } = *source else {
                        panic!("expected the nested command's original player");
                    };
                    assert_eq!(
                        (player.uuid(), permission),
                        (B_UUID, PermissionLevel::Owners)
                    );
                    assert_eq!(command.player(), Some(player));
                    assert_eq!(command.position, [3.5, 64.0, 6.0]);
                    assert_eq!(command.dimension, Dim::Overworld.key());
                    assert_eq!(host.players().acting_player(), Some(player));
                    let displaced = mod_api::__private::game::player_ref(
                        mod_api::EntityRef::new(player.entity().epoch(), A),
                        A_UUID,
                    );
                    assert!(!displaced.entity().run_command(
                        host,
                        "checknested",
                        PermissionLevel::Owners,
                        true
                    ));
                    events.lock().unwrap().push("nested");
                    Ok(1)
                }),
            );
    }

    fn right_click_block(
        &self,
        host: HostSide<'_>,
        event: &mut mod_api::RightClickBlockEvent<'_>,
    ) -> Flow {
        let HostSide::Server(host) = host else {
            panic!("the click is on the server");
        };
        assert_eq!(event.click.pos, HIT);
        assert_eq!(event.click.player, A_UUID);
        let actor = host.players().acting_player().unwrap();
        assert_eq!(actor.uuid(), A_UUID);
        let parked = host.level(Dim::Nether.id()).unwrap();
        assert_eq!(
            parked.block_state(host, HIT).unwrap().block().id(host),
            Some("minecraft:gold_block")
        );
        self.events.lock().unwrap().push("click");
        let b = host.player(B_UUID).unwrap();
        assert!(
            b.entity()
                .run_command(host, "checknested", PermissionLevel::Owners, true)
        );
        assert_eq!(host.players().acting_player(), Some(actor));
        assert_eq!(actor.level(host).unwrap().dimension(), Dim::Overworld.key());
        assert_eq!(host.level(Dim::Nether.id()), Some(parked));
        self.events.lock().unwrap().push("restored");
        if self.suppress.load(Ordering::Relaxed) {
            *event.use_item = TriState::False;
        } else {
            host.living(actor.entity())
                .unwrap()
                .set_hand_item(
                    host,
                    mod_api::Hand::Main,
                    Some(ItemStack::new("minecraft:dirt", 5).try_into_api().unwrap()),
                )
                .unwrap();
        }
        Flow::Continue
    }
}

#[test]
#[ignore = "daily"]
fn placement_packet_restores_actor_and_source_and_corrects_prediction() {
    run_with_game_stack(|| {
        let (events, suppress) = (
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(AtomicBool::new(false)),
        );
        let mods = vec![ModEntry::new(
            "observe-placement",
            Box::new(BlockListener {
                suppress: suppress.clone(),
                events: events.clone(),
            }),
        )]
        .into();
        mod_platform::testing::with_mods(mods, || {
            with_game_role(fixture_root(), ExecutionRole::IntegratedServer, |game| {
                let scratch = tempfile::tempdir().unwrap();
                game.server.world_root = scratch.path().to_path_buf();
                game.ensure_datapacks();
                game.build_registries();
                open_fixture_hub(game);
                game.register_native_commands();
                let out = add_player(game, A);
                let _watcher = add_player(game, B);
                game.enter_level(Dim::Nether).unwrap();
                prepare_entity_ticking_neighborhood(game, ChunkPos::new(0, 0));
                game.set_state_named(HIT, "minecraft:gold_block", &[])
                    .unwrap();
                game.enter_level(Dim::Overworld).unwrap();
                prepare_entity_ticking_neighborhood(game, ChunkPos::new(0, 0));
                game.set_state_named(HIT, "minecraft:stone", &[]).unwrap();
                game.server
                    .lan
                    .as_mut()
                    .unwrap()
                    .hub
                    .update_guest(A, |guest| {
                        guest.inventory[0] = Some(ItemStack::new("minecraft:dirt", 2));
                    })
                    .unwrap();

                use_on(game, [15, 64, 4], 1);
                assert!(events.lock().unwrap().is_empty());
                let packets = out();
                assert_eq!(
                    packets.iter().find_map(|p| receipt(p, 1)),
                    Some([false, false])
                );
                assert!(!packets.iter().any(|p| matches!(p, C::BlockUpdate(_))));

                let size = game.common.border.size();
                game.common.border.set_size(2.0).unwrap();
                use_on(game, HIT, 2);
                assert!(events.lock().unwrap().is_empty());
                let packets = out();
                let correction = packet_index(
                    &packets,
                    |p| matches!(p, C::BlockUpdate(p) if p.pos == BESIDE),
                );
                assert!(
                    correction < packet_index(&packets, |p| receipt(p, 2) == Some([false, false]))
                );
                game.common.border.set_size(size).unwrap();

                suppress.store(true, Ordering::Relaxed);
                use_on(game, HIT, 3);
                assert_eq!(
                    *events.lock().unwrap(),
                    ["click", "nested", "restored", "block"]
                );
                assert_eq!(&*game.block_name(BESIDE), "minecraft:air");
                events.lock().unwrap().clear();
                suppress.store(false, Ordering::Relaxed);

                let _recording = Recording::start();
                let source = game.server_source();
                game.with_source(source, |game| {
                    assert!(matches!(
                        game.native_command_origin(0).unwrap(),
                        CommandSource::Console { .. }
                    ));
                    use_on(game, HIT, 4);
                    assert!(matches!(
                        game.native_command_origin(0).unwrap(),
                        CommandSource::Console { .. }
                    ));
                });
                assert!(game.server.command_source.is_none());
                assert!(game.server.lan_acting.is_none());
                assert_eq!(
                    *events.lock().unwrap(),
                    ["click", "nested", "restored", "block"]
                );
                assert_eq!(&*game.block_name(BESIDE), "minecraft:dirt");
                let host = game.server.lan.as_ref().unwrap();
                assert_eq!(
                    host.hub.guest(A).unwrap().inventory[0],
                    Some(ItemStack::new("minecraft:dirt", 4))
                );
                SEEN.with_borrow(|seen| {
                    let outer = seen
                        .iter()
                        .filter(|(kind, _)| kind == "minecraft:any_block_use")
                        .collect::<Vec<_>>();
                    assert_eq!(outer.len(), 1);
                    assert_eq!(outer[0].1, Some(ItemStack::new("minecraft:dirt", 1)));
                });

                let before = out();
                let replied = packet_index(&before, |p| receipt(p, 4) == Some([true, false]));
                let dirt = game
                    .pack
                    .assets
                    .registry
                    .default_state("minecraft:dirt")
                    .unwrap();
                let correction = packet_index(
                    &before,
                    |p| matches!(p, C::BlockUpdate(p) if p.pos == BESIDE && p.state == dirt.0 as i32),
                );
                assert!(replied < correction);
                game.tick_lan(true);
                let after = out();
                let ack = packet_index(
                    &after,
                    |p| matches!(p, C::BlockChangedAck(p) if p.sequence == 4),
                );
                assert!(correction < ack);

                game.as_guest(A, |game| {
                    game.as_guest(B, |game| {
                        let i = game.guest_index(A).unwrap();
                        assert_eq!(game.as_guest_at(i, A, |_| ()), Err(TurnRefused::Nested));
                    })
                    .unwrap();
                    assert_eq!(game.server.lan_acting, Some(A));
                })
                .unwrap();
            });
        });
    });
}
