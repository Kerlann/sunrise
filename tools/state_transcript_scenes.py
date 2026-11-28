"""Command-harness scenes for the overworld state transcript."""

import json

from tools.state_digest_scene import AREA, SYNC, Y, entity_cluster_commands, item

SEED = 12345
DISTANCE = 4
SCENES = ('base', 'riding')
SPECIES = ('pig', 'cow', 'sheep', 'chicken', 'rabbit', 'zombie', 'skeleton', 'spider', 'wolf')
TICKS = 200


def cmd(command):
    return {'op': 'cmd', 'cmd': '/' + command}


def baseline():
    x0, z0, x1, z1 = AREA
    chunks = [[x, z] for z in range(z0 >> 4, (z1 >> 4) + 1) for x in range(x0 >> 4, (x1 >> 4) + 1)]
    return [
        cmd('difficulty peaceful'),
        cmd('time set night'),
        cmd('gamerule spawn_mobs false'),
        cmd('gamerule spawn_wandering_traders false'),
        cmd('gamerule advance_weather false'),
        cmd('gamerule random_tick_speed 0'),
        cmd(f'tp @s 44.5 {Y} -39.5 0 0'),
        cmd('gamemode creative'),
        {'op': 'settle'},
        cmd(f'forceload add {x0} {z0} {x1} {z1}'),
        {'op': 'wait_chunks', 'chunks': chunks},
        {'op': 'settle', 'required_chunks': chunks},
        {'op': 'align_game_time', 'target': 1000},
        cmd('difficulty hard'),
        {'op': 'sync_randoms', 'uniquifier': SYNC[0], 'entity_id': SYNC[1], 'level_seed': SYNC[2],
         'rand_value': 19390496, 'player_seed': 24681357},
        {'op': 'state_transcript', 'on': True},
    ]


def base_commands():
    for species_index, creature in enumerate(SPECIES):
        for i in range(20):
            x = 8 + species_index * 8 + (i % 4) * 1.5
            z = 8 + (i // 4) * 1.5
            yield f'summon minecraft:{creature} {x:.1f} {Y} {z:.1f}'
    for i in range(20):
        x = 8 + i * 4
        yield f'setblock {x} {Y} 60 minecraft:chest'
        yield (f'setblock {x} {Y + 1} 60 minecraft:hopper[facing=down]'
               '{Items:[{Slot:0b,id:"minecraft:cobblestone",count:64},{Slot:1b,id:"minecraft:dirt",count:32}]}')
    for i in range(5):
        yield f'summon minecraft:tnt {10 + i * 15}.5 {Y} 85.5 {{fuse:{300 + i * 10}s}}'
    for i in range(10):
        yield item(f'{8 + i * 8}.5', Y + 3, '70.5', 'diamond' if i % 2 else 'oak_log', i + 1)


def riding_commands():
    yield from entity_cluster_commands()
    yield (f'summon minecraft:birch_boat 35.5 {Y} 29.5 '
           '{Passengers:[{id:"minecraft:pig",NoAI:1b}]}')
    yield f'setblock 39 {Y} 29 minecraft:rail[shape=north_south]'
    yield (f'summon minecraft:minecart 39.5 {Y} 29.5 '
           '{Passengers:[{id:"minecraft:rabbit",NoAI:1b}]}')
    yield f'summon minecraft:pig 40.5 {Y} 35.5 {{NoAI:1b,Tags:["transcript_dies"]}}'


def scene_ops(name):
    if name not in SCENES:
        raise ValueError(f'unknown scene {name!r}')
    yield from baseline()
    commands = base_commands() if name == 'base' else riding_commands()
    yield from map(cmd, commands)
    for step in range(TICKS):
        if name == 'riding':
            if step == 40:
                yield cmd('ride @e[type=minecraft:sheep,limit=1] dismount')
            if step == 80:
                yield cmd('kill @e[tag=transcript_dies,limit=1]')
            if step == 120:
                yield cmd(f'summon minecraft:cow 44.5 {Y} 35.5 {{NoAI:1b}}')
        yield {'op': 'tick', 'n': 1}


def scene_script(name):
    return ''.join(json.dumps(op, separators=(',', ':')) + '\n' for op in scene_ops(name))


def expected_points(ops):
    enabled = False
    step, ordinal = 0, 0
    for line, op in enumerate(ops, 1):
        kind = op['op']
        if kind == 'state_transcript':
            enabled = op['on']
        elif enabled:
            if kind == 'tick':
                for _ in range(op['n']):
                    step += 1
                    yield step, 0, 'after_tick', line
                ordinal = 1
            elif kind in ('cmd', 'sync_randoms'):
                yield step, ordinal, 'after_op', line
                ordinal += 1
            else:
                raise ValueError(f'script line {line}: unexpected recorded op {kind!r}')
