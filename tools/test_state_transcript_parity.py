#!/usr/bin/env python3
"""Run the overworld state transcript scenes by hand (PARITY_MANUAL).

Pass --repeat to require fresh runs of each engine to be byte-identical before
the cross-engine comparison. The comparator's differences are valid results.
"""

import argparse
import gzip
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
from itertools import zip_longest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from tools import lab, oracle_harness, seed_agent, state_transcript_compare as transcript
from tools import state_transcript_scenes as scenes, test_rust_parity as base
from tools.integration_env import run_game


PARITY_MANUAL = 'python3 tools/test_state_transcript_parity.py --scene base --repeat'
JAVA_CLASS = 'VanillaCommandExecOracle'
TIMEOUT = 900


def noise_dimension(dimension, settings, source):
    return {'type': f'minecraft:{dimension}', 'generator': {
        'type': 'minecraft:noise', 'settings': f'minecraft:{settings}', 'biome_source': source,
    }}


def flat_fixture():
    return {
        'seed': scenes.SEED,
        'generate_structures': False,
        'bonus_chest': False,
        'dimensions': {
            'minecraft:overworld': {
                'type': 'minecraft:overworld',
                'generator': {
                    'type': 'minecraft:flat',
                    'settings': {
                        'biome': 'minecraft:plains', 'features': False, 'lakes': False,
                        'layers': [{'block': f'minecraft:{block}', 'height': height}
                                   for block, height in [('bedrock', 1), ('dirt', 2), ('grass_block', 1)]],
                        'structure_overrides': ['minecraft:strongholds', 'minecraft:villages'],
                    },
                },
            },
            'minecraft:the_nether': noise_dimension(
                'the_nether', 'nether', {'type': 'minecraft:multi_noise', 'preset': 'minecraft:nether'}),
            'minecraft:the_end': noise_dimension('the_end', 'end', {'type': 'minecraft:the_end'}),
        },
    }


def nbt_string(value):
    data = value.encode('ascii')
    return struct.pack('>H', len(data)) + data


def nbt_tag(value):
    if isinstance(value, bool):
        return 1, bytes([value])
    if isinstance(value, str):
        return 8, nbt_string(value)
    if isinstance(value, list):
        tags = [nbt_tag(item) for item in value]
        kind = tags[0][0]
        return 9, struct.pack('>Bi', kind, len(tags)) + b''.join(data for _, data in tags)
    if isinstance(value, dict):
        return 10, b''.join(nbt_named(name, item) for name, item in value.items()) + b'\0'
    return 3, struct.pack('>i', value)


def nbt_named(name, value):
    if name == 'seed':
        kind, data = 4, struct.pack('>q', value)
    else:
        kind, data = nbt_tag(value)
    return bytes([kind]) + nbt_string(name) + data


def write_sunrise_fixture(game, fixture, data_version):
    path = game / 'saves/harness/data/minecraft/world_gen_settings.dat'
    path.parent.mkdir(parents=True)
    path.write_bytes(gzip.compress(nbt_named('', {'data': fixture, 'DataVersion': data_version}), mtime=0))


def run_process(command, work, name, env, game=False):
    log_path = work / f'{name}.log'
    runner = run_game if game else subprocess.run
    with log_path.open('w', encoding='utf-8') as log:
        try:
            runner(list(map(str, command)), cwd=work, env=env, timeout=TIMEOUT,
                   stdout=log, stderr=subprocess.STDOUT, check=True)
        except (subprocess.SubprocessError, OSError) as error:
            raise ValueError(f'{name} failed ({error}); see {log_path}') from error


def vanilla_command(build, env):
    lock, jars, libraries = base.verify_artifacts()
    java_major = lock['minecraft']['java_major']
    classes = build / 'classes'
    classes.mkdir()
    source = lab.ROOT / 'mods/sunrise/src/test/java'
    oracle = source / 'dev/sunriselab/parity' / f'{JAVA_CLASS}.java'
    dependencies = os.pathsep.join(map(str, [jars['server'], *libraries]))
    oracle_harness.run_logged(
        ['javac', '--release', java_major, '-proc:none', '-sourcepath', source,
         '-cp', dependencies, '-d', classes, oracle],
        build, 'compile-java', env=env, timeout=TIMEOUT,
    )
    agent = seed_agent.java_option(build, java_major, env=env)
    return ['java', '-Xmx3G', '-Djava.awt.headless=true', '-Duser.timezone=UTC', agent,
            '-Dsunrise.parity.flat=true', f'-Dsunrise.parity.distance={scenes.DISTANCE}',
            '-cp', os.pathsep.join([str(classes), dependencies])]


def sunrise_binary(build, binary):
    if binary:
        return binary.resolve(strict=True)
    hz = Path.home() / 'hz-dev/hz'
    cargo = [hz, 'cargo'] if hz.is_file() else ['cargo']
    oracle_harness.run_logged(
        [*cargo, 'build', '--locked', '--profile', 'fast', '-p', 'sunrise-game',
         '--bin', 'sunrise-client', '--features', 'sunrise-client/state-digest'],
        build, 'compile-rust', timeout=TIMEOUT,
    )
    metadata = json.loads(subprocess.check_output(
        [*cargo, 'metadata', '--locked', '--no-deps', '--format-version', '1'],
        cwd=lab.ROOT, timeout=TIMEOUT,
    ))
    return lab.binary(Path(metadata['target_directory']) / 'fast', 'sunrise-client').resolve(strict=True)


def expected_command_results(ops, engine):
    for op in ops:
        kind = op['op']
        if kind == 'cmd':
            yield op['cmd'], None
        elif kind == 'server_distances':
            yield '#server_distances', [[True, scenes.DISTANCE]] * 2
        elif kind == 'chunk_ready':
            x, z = op['chunk']
            result_count = 4 if engine == 'sunrise' else 3
            yield f'#chunk_ready {x} {z}', [[True, 1]] * result_count


def check_commands(path, ops, engine):
    expected = expected_command_results(ops, engine)
    with path.open(encoding='utf-8') as source:
        for number, (encoded, expectation) in enumerate(zip_longest(source, expected), 1):
            if encoded is None or expectation is None:
                raise ValueError(f'{path}:{number}: command output count differs from the script')
            command, expected_results = expectation
            row = json.loads(encoded)
            if row.get('cmd') != command:
                raise ValueError(f'{path}:{number}: expected output for {command!r}')
            results = row.get('results', [])
            if expected_results is not None and results != expected_results:
                raise ValueError(f'{path}:{number}: {command}: expected {expected_results}, got {results}')
            if not any(result[0] is True for result in results):
                raise ValueError(f'{path}:{number}: command failed: {command}')
            if command == '#server_distances':
                print(f'{engine} verified view and simulation distance {scenes.DISTANCE}', flush=True)

def check_transcript(path, engine, ops, script_hash, fixture_hash):
    run = transcript.read_transcript(path)
    for name, value in [('engine', engine), ('script_sha256', script_hash), ('fixture_sha256', fixture_hash)]:
        if run.header.fields[name] != value:
            raise transcript.InvalidTranscript(f'{path}:1: expected {name}={value}, got {run.header.fields[name]}')
    for name in transcript.DISTANCES:
        value = run.header.fields.get(name)
        if value is not None and value != str(scenes.DISTANCE):
            raise transcript.InvalidTranscript(f'{path}:1: expected {name}={scenes.DISTANCE}, got {value}')
    for point, expected in zip_longest(run.points, scenes.expected_points(ops)):
        actual = (*point.key, int(point.row.fields['op'])) if point else None
        if actual != expected:
            line = point.row.line if point else run.footer.line
            raise transcript.InvalidTranscript(f'{path}:{line}: expected point {expected}, got {actual}')
    baseline = run.points[0]
    day_time = baseline.level.fields['day_time']
    if day_time != str(scenes.NIGHT):
        raise transcript.InvalidTranscript(
            f'{path}:{baseline.level.line}: expected baseline day_time={scenes.NIGHT}, got {day_time}'
        )
    if [member.fields['type'] for member in baseline.members] != ['minecraft:player']:
        raise transcript.InvalidTranscript(
            f'{path}:{baseline.level.line}: expected only the player before scene summons, '
            f'found {[member.identity for member in baseline.members]}'
        )
    return run


def run_engine(engine, command, build, attempt, script, ops, env, fixture, fixture_hash):
    work = build / f'{engine}-{attempt}'
    work.mkdir()
    commands = work / 'commands.jsonl'
    output = work / 'transcript.txt'
    if engine == 'vanilla':
        jvm = [*command, f'-Dsunrise.stateTranscript={output}',
               f'-Dsunrise.stateTranscriptFixtureSha256={fixture_hash}']
        run_process([*jvm, f'dev.sunriselab.parity.{JAVA_CLASS}', work / 'universe',
                     script, commands, scenes.SEED], work, engine, env)
    else:
        data_version = lab.version_info()['world_version']
        write_sunrise_fixture(work, fixture, data_version)
        game_env = lab.game_env(
            env, **seed_agent.ENV, SUNRISE_PLAYER_NAME='Player', SUNRISE_COMMAND_SCRIPT=script,
            SUNRISE_COMMAND_OUTPUT=commands, SUNRISE_STATE_TRANSCRIPT=output,
            SUNRISE_STATE_TRANSCRIPT_FIXTURE_SHA256=fixture_hash,
            SUNRISE_STATE_TRANSCRIPT_SIMULATION_DISTANCE=scenes.DISTANCE,
            SUNRISE_API_DISCOVERY='http://127.0.0.1:9/',
        )
        run_process([command, '--resources', lab.RESOURCES, '--game-dir', work, '--world', 'harness',
                     '--seed', scenes.SEED, '--radius', scenes.DISTANCE], work, engine, game_env, game=True)
    check_commands(commands, ops, engine)
    return check_transcript(output, engine, ops, lab.digest(script, 'sha256'), fixture_hash)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scene', choices=scenes.SCENES, default='base')
    parser.add_argument('--repeat', action='store_true', help='require two byte-identical fresh runs per engine')
    parser.add_argument('--only', choices=('vanilla', 'sunrise'))
    parser.add_argument('--binary', type=Path, help='use an existing state-digest Sunrise client')
    args = parser.parse_args(argv)

    root = lab.LOCAL / 'state_transcript' / args.scene
    root.mkdir(parents=True, exist_ok=True)
    build = Path(tempfile.mkdtemp(prefix='run-', dir=root))
    print(f'State transcript output in {build}', flush=True)
    script = build / 'script.jsonl'
    script.write_text(scenes.scene_script(args.scene), encoding='utf-8', newline='\n')
    ops = [json.loads(line) for line in script.read_text(encoding='utf-8').splitlines()]
    fixture = flat_fixture()
    manifest = build / 'fixture.json'
    lab.write_json(manifest, {'preset': 'minecraft:flat', 'view_distance': scenes.DISTANCE,
                              'simulation_distance': scenes.DISTANCE, 'world': fixture})
    fixture_hash = lab.digest(manifest, 'sha256')
    env = dict(os.environ, PARITY_JAVA_CACHE='0', DISPLAY=':99', SUNRISE_NO_AUDIO='1')
    for name in ('WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'SUNRISE_HARNESS_LOADED_FIXTURE',
                 'SUNRISE_HARNESS_TERRAIN_ONLY', 'SUNRISE_SESSION_FILE'):
        env.pop(name, None)
    try:
        runs = []
        engines = (args.only,) if args.only else ('vanilla', 'sunrise')
        for engine in engines:
            command = vanilla_command(build, env) if engine == 'vanilla' else sunrise_binary(build, args.binary)
            first = run_engine(engine, command, build, 1, script, ops, env, fixture, fixture_hash)
            if args.repeat:
                second = run_engine(engine, command, build, 2, script, ops, env, fixture, fixture_hash)
                if first.path.read_bytes() != second.path.read_bytes():
                    print(f'{engine} repeatability failed', flush=True)
                    transcript.report_comparison(first, second, json_path=build / f'{engine}-repeat.json')
                    return 1
                print(f'{engine} repeated byte for byte', flush=True)
            runs.append(first)
        if args.only:
            print(runs[0].path)
            return 0
        return transcript.report_comparison(*runs, json_path=build / 'report.json')
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f'State transcript run failed: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
