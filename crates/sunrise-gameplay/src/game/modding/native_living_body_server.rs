use super::commands::Target;
use crate::game::ServerGame;

impl ServerGame<'_> {
    /// The entity id `who` names as a hit's causing entity: a mob's own, the player in place's
    /// for `Who::Player`; 0 for none.
    pub(in crate::game) fn causer_id(&self, who: Option<sunrise_sim::mobs::Who>) -> i32 {
        match who {
            Some(sunrise_sim::mobs::Who::Mob(id)) => id as i32,
            Some(sunrise_sim::mobs::Who::Player) => {
                self.common.player.as_ref().map_or(0, |p| p.entity_id)
            }
            None => 0,
        }
    }
    /// The guest whose entity id `id` is, when the `LivingRef` reads answer for it from its hub
    /// record now (`Target::Guest`, not the player in place): the causing entity a hit may lend
    /// read-only ([`HitLend::parked_guest`](sunrise_client::health::HitLend::parked_guest)).
    /// None for anything else, so a hit's view never answers where `LivingRef` does not.
    pub(in crate::game) fn native_parked_causer(&self, id: i32) -> Option<u32> {
        let epoch = self.common.native_lifecycle.epoch()?;
        let living = mod_api::LivingRef::new(mod_api::EntityRef::new(
            epoch,
            sunrise_gameplay_common::game::entity_ids::entity_id_bits(id),
        ));
        match self.native_living_target(living)? {
            Target::Guest(conn) if self.server.lan_acting != Some(conn) => Some(conn),
            _ => None,
        }
    }
}

#[cfg(test)]
mod issue_6336_tests {
    use std::sync::{Arc, Mutex, mpsc};

    use mod_api::{Flow, HitHost, Hooks, LivingIncomingDamageEvent, ModEntry, Mods};
    use sunrise_gameplay_common::game::{creatures::KnownCause, entity_ids::entity_id_bits};
    use sunrise_protocol::packets::configuration::ClientInformation;
    use sunrise_sim::lan::{Joined, PlayerSender, guest::Guest};

    use crate::game::{
        native_entity_refs::tests::with_game,
        native_guest_turn_tests::{open_fixture_hub, place_host},
    };

    /// A guest id with the high bit set is stored as a negative `i32`; reading
    /// it back must keep every bit, not refuse the id (#6336).
    #[test]
    fn issue_6336_a_high_bit_guest_id_reads_back_as_its_bits() {
        let wire = 0x8000_0001_u32 as i32;
        assert_eq!(entity_id_bits(wire), 0x8000_0001);
        assert_eq!(entity_id_bits(-1), u32::MAX);
    }

    #[test]
    #[ignore = "daily"]
    fn a_high_bit_guest_id_survives_both_hit_views() {
        const NAME: &str = "game::server_side::native_living_body_server::issue_6336_tests::a_high_bit_guest_id_survives_both_hit_views";
        const GUEST_ID: u32 = 0x8000_0001;
        const CONN: u32 = 12;

        struct RecordHitViews(Arc<Mutex<Vec<[Option<u32>; 2]>>>);

        #[mod_api::__subscriptions]
        impl Hooks for RecordHitViews {
            fn living_incoming_damage(
                &self,
                hit: &mut dyn HitHost,
                _: &mut LivingIncomingDamageEvent<'_>,
            ) -> Flow {
                self.0.lock().unwrap().push([
                    hit.victim().map(|view| view.id()),
                    hit.causing().map(|view| view.id()),
                ]);
                Flow::Cancel
            }
        }

        sunrise_gameplay_common::game::in_own_process(NAME, || {
            mod_platform::__private::test_platform::install();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let mods: Mods = vec![ModEntry::new(
                "record-hit-views",
                Box::new(RecordHitViews(Arc::clone(&seen))),
            )]
            .into();
            mod_platform::testing::with_mods(mods, || {
                with_game(|game| {
                    place_host(game);
                    open_fixture_hub(game);
                    let (tx, _rx) = mpsc::channel();
                    let mut guest = Guest::new(
                        Joined {
                            conn: CONN,
                            uuid: u128::from(CONN),
                            name: "guest12".into(),
                            properties: Vec::new(),
                            info: ClientInformation::new(8),
                            brand: String::new(),
                            sender: PlayerSender::new(tx, Arc::default()),
                            latency: Arc::default(),
                            address: "127.0.0.1:1".parse().unwrap(),
                            expected: false,
                        },
                        GUEST_ID as i32,
                        [3.0, 64.0, 5.0],
                        10,
                    );
                    guest.placed = true;
                    guest.loaded = true;
                    guest.load_timeout = 0;
                    game.server.lan.as_mut().unwrap().hub.guests.push(guest);
                    game.reset_entity_residency();

                    // DamageSource.getEntity names the parked guest while the host takes the hit.
                    let _ = game.hurt_player_from(
                        1.0,
                        None,
                        "minecraft:generic",
                        KnownCause::Entity(GUEST_ID),
                        false,
                    );
                    // The next hit reads Entity.getId from the guest's acting victim body.
                    game.as_guest(CONN, |game| game.hurt_player_by("minecraft:generic", 1.0))
                        .unwrap();
                });
            });
            assert_eq!(
                seen.lock().unwrap().as_slice(),
                [[Some(99), Some(GUEST_ID)], [Some(GUEST_ID), None]],
            );
        });
    }
}
