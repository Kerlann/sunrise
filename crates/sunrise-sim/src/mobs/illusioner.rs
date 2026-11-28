//! The illusioner (`Illusioner`): a bow-wielding spellcaster that turns
//! invisible behind four mirror images and blinds its target.
use super::{
    Mob, MobEvent, Species, Who,
    goals::{
        Avoid, AvoidEntity, Cx, Float, Goal, HurtByTarget, LookAtPlayer, NearestTarget, Stroll,
        goal_clone, reduced,
    },
    illager::CastingSpell,
    undead::BowAttack,
};
use sunrise_core::random::RandomSource;

const INVISIBILITY: &str = "minecraft:invisibility";
const BLINDNESS: &str = "minecraft:blindness";
/// `Illusioner.NUM_ILLUSIONS`.
pub const COPIES: usize = 4;

/// `Illusioner.clientSideIllusionTicks` and the two offset sets.
#[derive(Clone, Debug, Default)]
pub struct Illusions {
    pub ticks: i32,
    pub offsets: [[[f64; 3]; COPIES]; 2],
}

// `IllagerSpell` ids: DISAPPEAR (the mirror spell) 4, BLINDNESS 5.
pub const MIRROR: u8 = 4;
pub const BLINDNESS_SPELL: u8 = 5;

/// `Illusioner.performRangedAttack`: the skeleton's arrow (`undead::bow_shot`), shot from
/// `BowAttack`. Not through the skeleton's replaceable `undead::perform_ranged_attack`, which
/// a mod's row on `AbstractSkeleton.performRangedAttack` takes.
pub(super) fn perform_ranged_attack(m: &mut Mob, who: Who, cx: &mut Cx, power: f32) {
    super::undead::bow_shot(m, who, cx, power);
}
/// `Illusioner.registerGoals`.
pub fn register(m: &mut Mob) {
    let g = &mut m.ai.goals;
    g.add(0, Float);
    g.add(1, CastingSpell);
    g.add(
        3,
        AvoidEntity::new(Avoid::Species(vec![Species::Creaking]), 8.0, 1.0, 1.2),
    );
    g.add(4, Spell::new(MIRROR));
    g.add(5, Spell::new(BLINDNESS_SPELL));
    g.add(6, BowAttack::new(0.5, 20, 15.0));
    g.add(8, Stroll::new(0.6));
    g.add(9, LookAtPlayer::new(3.0).probability(1.0));
    g.add(10, LookAtPlayer::mobs(8.0));
    let mut hurt = HurtByTarget::new().alert();
    hurt.ignore = vec![
        Species::Pillager,
        Species::Vindicator,
        Species::Witch,
        Species::Evoker,
        Species::Illusioner,
        Species::Vex,
        Species::Ravager,
    ];
    m.ai.targets.add(1, hurt);
    m.ai.targets
        .add(2, NearestTarget::player(true).unseen_memory(300));
    m.ai.targets.add(
        3,
        NearestTarget::species(&[Species::Villager, Species::WanderingTrader], false)
            .unseen_memory(300),
    );
    m.ai.targets.add(
        3,
        NearestTarget::species(&[Species::IronGolem], false).unseen_memory(300),
    );
}

/// `Illusioner.finalizeSpawn`: a bow in the main hand.
pub fn finalize(m: &mut Mob) {
    m.extra.set_main_hand(Some("minecraft:bow".into()));
}

/// `IllusionerMirrorSpellGoal` and `IllusionerBlindnessSpellGoal`.
#[derive(Clone, Debug)]
struct Spell {
    spell: u8,
    warmup: i32,
    next: i64,
    last_target: Option<Who>,
}
impl Spell {
    fn new(spell: u8) -> Self {
        Self {
            spell,
            warmup: 0,
            next: 0,
            last_target: None,
        }
    }
    /// (casting time, interval, prepare sound).
    fn timing(&self) -> (i32, i64, &'static str) {
        if self.spell == MIRROR {
            (20, 340, "entity.illusioner.prepare_mirror")
        } else {
            (20, 180, "entity.illusioner.prepare_blindness")
        }
    }
}
impl Goal for Spell {
    goal_clone!();
    #[inline]
    fn flags(&self) -> u8 {
        0
    }
    #[inline]
    fn can_use(&mut self, m: &mut Mob, cx: &mut Cx) -> bool {
        if !m.target_info(cx.w).is_some_and(|t| t.alive)
            || m.extra.counter > 0
            || i64::from(m.age) < self.next
        {
            return false;
        }
        if self.spell == MIRROR {
            !m.has_effect(INVISIBILITY)
        } else {
            m.ai.target != self.last_target && cx.w.difficulty.effective > 2.0
        }
    }
    #[inline]
    fn can_continue(&mut self, m: &mut Mob, cx: &mut Cx) -> bool {
        m.target_info(cx.w).is_some_and(|t| t.alive) && self.warmup > 0
    }
    #[inline]
    fn start(&mut self, m: &mut Mob, cx: &mut Cx) {
        let (casting, interval, sound) = self.timing();
        self.warmup = reduced(20);
        m.extra.counter = casting;
        self.next = i64::from(m.age) + interval;
        cx.events.push(MobEvent::Sound(sound, m.center(), 1.0, 1.0));
        m.extra.variant2 = self.spell;
        if self.spell == BLINDNESS_SPELL {
            self.last_target = m.ai.target;
        }
    }
    #[inline]
    fn tick(&mut self, m: &mut Mob, cx: &mut Cx) {
        self.warmup -= 1;
        if self.warmup != 0 {
            return;
        }
        if self.spell == MIRROR {
            m.add_effect(INVISIBILITY, 1200, 0);
        } else {
            // `performSpellCasting` blinds the target whatever it is: a
            // villager or an iron golem as well as the player.
            match m.ai.target {
                Some(Who::Player) => {
                    cx.events
                        .push(cx.w.route_player_event(MobEvent::EffectPlayer {
                            effect: BLINDNESS,
                            ticks: 400,
                            amplifier: 0,
                            ambient: false,
                        }))
                }
                Some(Who::Mob(target)) => cx.events.push(MobEvent::EffectMob {
                    target,
                    effect: BLINDNESS,
                    ticks: 400,
                    amplifier: 0,
                }),
                None => {}
            }
        }
        cx.events.push(MobEvent::Sound(
            "entity.illusioner.cast_spell",
            m.center(),
            1.0,
            1.0,
        ));
    }
}

/// `Entity.isInvisible` of an illusioner on a client: the shared flag stands
/// in for the effect the server holds, which draws the mirror images.
pub fn set_invisible(m: &mut Mob, on: bool) {
    m.retain_effects(|e| e.id != INVISIBILITY);
    if on {
        m.add_effect(INVISIBILITY, i32::MAX, 0);
    }
}

/// The client-side half of `Illusioner.aiStep`: new image offsets when
/// hurt and once a minute, with a puff of cloud and a sound.
pub fn tick(m: &mut Mob, events: &mut Vec<MobEvent>) {
    if !m.has_effect(INVISIBILITY) {
        return;
    }
    if step(m) {
        events.push(MobEvent::Particles {
            kind: "minecraft:cloud",
            at: [m.position.x, m.position.y + 0.9, m.position.z],
            count: 16,
        });
        events.push(MobEvent::Sound(
            "entity.illusioner.mirror_move",
            m.center(),
            1.0,
            1.0,
        ));
    }
}

/// `Illusioner.aiStep` on a client of a server: invisible is the synched
/// shared flag. The puff is drawn here exactly (sixteen `addParticle`s); the
/// sound comes from the server.
pub fn client_tick(m: &mut Mob, events: &mut Vec<MobEvent>) {
    if !m.invisible || !step(m) {
        return;
    }
    let (w, h) = (f64::from(m.width()), f64::from(m.height()));
    for _ in 0..16 {
        // getRandomX(0.5), getRandomY() and getZ(0.5), which is not random.
        let x = m.position.x + w * (2.0 * m.random.next_double() - 1.0) * 0.5;
        let y = m.position.y + h * m.random.next_double();
        events.push(MobEvent::ParticleAt {
            kind: "cloud",
            at: [x, y, m.position.z + w * 0.5],
            velocity: [0.0; 3],
        });
    }
}

/// The image offsets' clock of an invisible illusioner. True when the images
/// jumped to new offsets (hurt, or once a minute), which the puff goes with.
fn step(m: &mut Mob) -> bool {
    let hurt = m.hurt_time;
    let age = m.age;
    let mut il = m.extra.illusions.take().unwrap_or_default();
    il.ticks = (il.ticks - 1).max(0);
    let jumped = hurt == 1 || age.is_multiple_of(1200);
    if jumped {
        il.ticks = 3;
        for i in 0..COPIES {
            il.offsets[0][i] = il.offsets[1][i];
            let r = &mut m.random;
            let x = (-6 + r.next_int_bound(13).unwrap_or(0)) as f64 * 0.5;
            let y = (r.next_int_bound(6).unwrap_or(0) - 4).max(0) as f64;
            let z = (-6 + r.next_int_bound(13).unwrap_or(0)) as f64 * 0.5;
            il.offsets[1][i] = [x, y, z];
        }
    } else if hurt == 9 {
        il.ticks = 3;
        for i in 0..COPIES {
            il.offsets[0][i] = il.offsets[1][i];
            il.offsets[1][i] = [0.0; 3];
        }
    }
    m.extra.illusions = Some(il);
    jumped
}

/// `Illusioner.getIllusionOffsets`.
pub fn offsets(m: &Mob, partial: f32) -> [[f64; 3]; COPIES] {
    let Some(il) = &m.extra.illusions else {
        return [[0.0; 3]; COPIES];
    };
    if il.ticks <= 0 {
        return il.offsets[1];
    }
    let scale = sunrise_core::jmath::pow(f64::from((il.ticks as f32 - partial) / 3.0), 0.25);
    let mut out = [[0.0; 3]; COPIES];
    for (i, o) in out.iter_mut().enumerate() {
        for k in 0..3 {
            o[k] = il.offsets[1][i][k] * (1.0 - scale) + il.offsets[0][i][k] * scale;
        }
    }
    out
}

/// Maximum horizontal mirror scatter from `Illusioner.aiStep` (`(-6 + nextInt(13)) * 0.5`).
pub const MAX_HORIZONTAL_SCATTER: f64 = 3.0;
/// Maximum upward vertical mirror scatter from `Illusioner.aiStep` (`max(nextInt(6) - 4, 0)`).
pub const MAX_VERTICAL_SCATTER: f64 = 1.0;
/// Horizontal wobble amplitude in `IllusionerRenderer.submit` (`Mth.cos(...) * 0.025`).
pub const HORIZONTAL_WOBBLE: f64 = 0.025;
/// Vertical wobble amplitude in `IllusionerRenderer.submit` (`Mth.cos(...) * 0.0125`).
pub const VERTICAL_WOBBLE: f64 = 0.0125;

/// The wobble `IllusionerRenderer.submit` adds to each image (`Mth.cos` quantized table).
pub fn wobble(i: usize, age: f32) -> [f64; 3] {
    let i = i as f32;
    [
        f64::from(sunrise_core::geometry::trig::cos(f64::from(i + age * 0.5))) * HORIZONTAL_WOBBLE,
        f64::from(sunrise_core::geometry::trig::cos(f64::from(i + age * 0.75))) * VERTICAL_WOBBLE,
        f64::from(sunrise_core::geometry::trig::cos(f64::from(i + age * 0.7))) * HORIZONTAL_WOBBLE,
    ]
}

/// Total mirror-image offsets (`Illusioner.getIllusionOffsets` plus `IllusionerRenderer.submit`
/// wobble), shared between the body model and held/head item passes.
pub fn mirror_offsets(m: &Mob, partial: f32) -> [[f64; 3]; COPIES] {
    let age = m.age as f32 + partial;
    let base = offsets(m, partial);
    std::array::from_fn(|i| {
        let w = wobble(i, age);
        [base[i][0] + w[0], base[i][1] + w[1], base[i][2] + w[2]]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settled_offsets_are_the_new_set() {
        let il = Illusions {
            ticks: 0,
            offsets: [[[1.0; 3]; COPIES], [[2.0; 3]; COPIES]],
        };
        let mut v = [[0.0; 3]; COPIES];
        v.copy_from_slice(&il.offsets[1]);
        assert_eq!(v[0], [2.0; 3]);
    }
    #[test]
    fn wobble_is_small() {
        assert!(wobble(1, 10.0).iter().all(|c| c.abs() <= 0.025));
    }

    /// Independent 26.3 bytecode expectations: `IllusionerRenderer.submit` uses quantized
    /// `Mth.cos(double)` for wobble and `Illusioner.getIllusionOffsets` divides `(ticks - partial)`
    /// by `3.0f` in `f32` before widening to `f64` for `Math.pow(..., 0.25)`.
    #[test]
    fn wobble_and_offsets_match_vanilla_26_3_math() {
        let want_wobble: [[u64; 3]; COPIES] = [
            [
                0x3f98_1e83_cccc_cccd,
                0xbf86_4f29_4ccc_cccd,
                0xbf99_5028_0000_0000,
            ],
            [
                0x3f77_3fda_1999_999a,
                0xbf47_c8bf_e666_6667,
                0xbf90_e43c_b333_3333,
            ],
            [
                0xbf91_d6f9_9999_999a,
                0x3f84_b39b_e666_6667,
                0x3f7c_3cc8_3333_3334,
            ],
            [
                0xbf99_16b9_4ccc_cccd,
                0x3f87_db6c_b333_3334,
                0x3f98_8536_1999_999a,
            ],
        ];
        for (i, want_bits) in want_wobble.into_iter().enumerate() {
            let got = wobble(i, 13.25);
            assert_eq!(got.map(f64::to_bits), want_bits, "wobble copy {i}");
        }

        let mob_type = std::sync::Arc::new(crate::mobs::MobType {
            type_key: Default::default(),
            id: "minecraft:illusioner".into(),
            species: crate::mobs::Species::Illusioner,
            width: 0.6,
            height: 1.95,
            eye_height: 1.62,
            category: "monster".into(),
            max_health: 32.0,
            speed: 0.5,
            flying_speed: 0.0,
            attack_damage: 0.0,
            attack_knockback: 0.0,
            follow_range: 18.0,
            step_height: 0.6,
            armor: 0.0,
            armor_toughness: 0.0,
            knockback_resistance: 0.0,
            jump_strength: 0.42,
            attributes: std::sync::Arc::default(),
            item_stack_limits: std::sync::Arc::default(),
            attribute_bounds: Default::default(),
            gravity: 0.08,
            safe_fall_distance: 3.0,
            fall_damage_multiplier: 1.0,
            water_efficiency: 0.0,
            water_slowdown: 0.8,
            air_speed: 0.02,
            baby_scale: 0.5,
            head_rot_speed: 10.0,
            max_head_x_rot: 40.0,
            max_head_y_rot: 75.0,
            ambient_interval: 120,
            flags: crate::mobs::Flags::default(),
            brain: crate::mobs::Brain::Passive,
            anim: crate::mobs::Anim::Illager,
            sounds: [""; 3],
            sound_overrides: Default::default(),
            food: Default::default(),
            tempt: Default::default(),
            passenger_attachments: Vec::new(),
        });
        let mut mob = Mob::new(1, mob_type, sunrise_core::geometry::Vec3::ZERO, 0.0);
        mob.age = 13;
        mob.extra.illusions = Some(Illusions {
            ticks: 2,
            offsets: [[[1.0, 0.0, 0.0]; COPIES], [[0.0, 0.0, 0.0]; COPIES]],
        });
        let scale_bits = offsets(&mob, 0.35)[0][0].to_bits();
        assert_eq!(scale_bits, 0x3feb_8ebb_cced_4ae9);
        let combined = mirror_offsets(&mob, 0.25);
        for (i, want_bits) in want_wobble.into_iter().enumerate() {
            let base = offsets(&mob, 0.25)[i];
            let w = want_bits.map(f64::from_bits);
            assert_eq!(
                combined[i],
                [base[0] + w[0], base[1] + w[1], base[2] + w[2]],
                "mirror_offsets copy {i}"
            );
        }
    }

    /// `SpellcasterIllager.customServerAiStep` counts a cast down once a
    /// tick for the illusioner as for the evoker (`Illusioner` has no override).
    /// Without it the count stayed at 20 after the first spell, so the
    /// illusioner kept casting: no second spell and no bow.
    #[test]
    #[ignore = "daily"]
    fn the_cast_counts_down() {
        use crate::mobs::world::with_test_world;
        use std::sync::Arc;
        let mut left = None;
        let ran = with_test_world(&[], |w, types| {
            let mut m = Mob::new(
                1,
                Arc::new(types.types["minecraft:illusioner"].clone()),
                sunrise_core::geometry::Vec3::new(0.5, 64.0, 0.5),
                0.0,
            );
            m.extra.counter = 20;
            m.random = sunrise_core::random::LegacyRandom::new(0);
            let mut fixture = crate::health::TestLevel::new(0);
            let mut level = fixture.level();
            let mut events = Vec::new();
            for _ in 0..5 {
                crate::mobs::ai::tick(&mut m, w, &mut level, &mut events);
            }
            left = Some(m.extra.counter);
        });
        if !ran {
            return;
        }
        assert_eq!(left, Some(15));
    }

    /// `IllusionerBlindnessSpellGoal.performSpellCasting` adds Blindness for
    /// 400 ticks to the `LivingEntity` the illusioner targets: a villager or
    /// an iron golem (mob 2) gets it as the player does. Without the fix the
    /// cast on a mob sent no effect at all.
    #[test]
    #[ignore = "daily"]
    fn the_blindness_spell_reaches_a_targeted_mob() {
        use crate::mobs::world::with_test_world;
        use std::sync::Arc;
        let mut cast = Vec::new();
        let ran = with_test_world(&[], |w, types| {
            for target in [Who::Mob(2), Who::Player] {
                let mut m = Mob::new(
                    1,
                    Arc::new(types.types["minecraft:illusioner"].clone()),
                    sunrise_core::geometry::Vec3::new(0.5, 64.0, 0.5),
                    0.0,
                );
                m.ai.target = Some(target);
                let mut spell = Spell::new(BLINDNESS_SPELL);
                spell.warmup = 1;
                m.random = sunrise_core::random::LegacyRandom::new(0);
                let mut fixture = crate::health::TestLevel::new(0);
                let mut level = fixture.level();
                let mut events = Vec::new();
                spell.tick(
                    &mut m,
                    &mut Cx {
                        w,
                        level: &mut level,
                        events: &mut events,
                    },
                );
                cast.push(events.into_iter().find_map(|e| match e {
                    MobEvent::EffectMob {
                        target,
                        effect,
                        ticks,
                        amplifier,
                    } => Some((Some(target), effect, ticks, amplifier)),
                    MobEvent::EffectPlayer {
                        effect,
                        ticks,
                        amplifier,
                        ..
                    } => Some((None, effect, ticks, amplifier)),
                    _ => None,
                }));
            }
        });
        if !ran {
            return;
        }
        assert_eq!(
            cast,
            [
                Some((Some(2), BLINDNESS, 400, 0)),
                Some((None, BLINDNESS, 400, 0)),
            ]
        );
    }
}
