"""Synthetic state transcript comparisons and command scene validation."""

from collections import Counter
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import json
from pathlib import Path
import tempfile
import unittest

from tools import state_transcript_compare as compare
from tools import state_transcript_scenes as scenes


PIG = ('1', 'minecraft:pig')
COW = ('2', 'minecraft:cow')
SHEEP = ('3', 'minecraft:sheep')
RANDOM = '000000012345,-'
UUID = '00000000-0000-0000-0000-000000000001'
ZERO = '0000000000000000'
POS = ','.join([ZERO] * 3)
HEADER = ('H\tformat=sunrise-state-transcript\tversion=1\tengine=sunrise'
          f'\tscript_sha256={"a" * 64}\tfixture_sha256={"b" * 64}\tbuild=test\tgame_version=26.3')


def row(kind, **fields):
    return '\t'.join([kind, *(f'{key}={value}' for key, value in fields.items())])


def point(step, members=(PIG, COW), turns=(), departures=(), changes=None, ordinal=0):
    changes = changes or {}
    phase = 'after_tick' if step else 'after_op'
    rows = [
        row('P', step=step, phase=phase, ord=ordinal, op=step + ordinal + 1,
            game_time=100 + step, dim='minecraft:overworld'),
        row('L', level_random=RANDOM, rand_value=0, last_entity_id=3, members=len(members)),
    ]
    for index, (entity_id, kind) in enumerate(members):
        fields = dict(pos=POS, motion=POS, rot='00000000,00000000', ground=1, tick_count=step,
                      fall_distance=ZERO, fire=0, air=300, invulnerable_time=0, no_gravity=0,
                      vehicle='-', passengers='-', random=RANDOM, health='41200000', uuid=UUID)
        fields.update(changes.get(entity_id, {}))
        rows.append(row('M', i=index, id=entity_id, type=kind, **fields))
    for index, (entity_id, kind) in enumerate(turns):
        rows.append(row('T', n=index, id=entity_id, type=kind, role='root', vehicle='-'))
    for index, ((entity_id, kind), reason) in enumerate(departures):
        rows.append(row('D', n=index, id=entity_id, type=kind, reason=reason))
    return rows


def transcript(*points):
    rows = [line for p in points for line in p]
    return '\n'.join([HEADER, *rows, row('Z', points=len(points), rows=len(rows)), ''])


class StateTranscriptCompareTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.left = Path(self.directory.name) / 'left.txt'
        self.right = Path(self.directory.name) / 'right.txt'

    def compare(self, left, right):
        self.left.write_text(left, encoding='utf-8')
        self.right.write_text(right, encoding='utf-8')
        return compare.compare(compare.read_transcript(self.left), compare.read_transcript(self.right))

    def cli(self, left, right, *args):
        self.left.write_text(left, encoding='utf-8')
        self.right.write_text(right, encoding='utf-8')
        output = StringIO()
        with redirect_stdout(output), redirect_stderr(output):
            code = compare.main([str(self.left), str(self.right), *args])
        return code, output.getvalue()

    def test_identical(self):
        data = transcript(point(0), point(1, turns=(PIG, COW)))
        report = self.compare(data, data)
        self.assertIsNone(report['earliest'])
        self.assertTrue(all(group['status'] == 'identical' for group in report['groups'].values()))
        self.assertEqual(self.cli(data, data)[0], 0)

    def test_member_present_on_one_side_only(self):
        data = transcript(point(0))
        report = self.compare(data, transcript(point(0, members=(PIG, COW, SHEEP))))
        group = report['groups']['ids']
        self.assertEqual(group['first']['entity'], SHEEP)
        self.assertEqual(group['first']['field'], 'unmatched')
        self.assertEqual(group['first']['left'], [])
        self.assertEqual(group['first']['right'], [SHEEP])
        self.assertEqual(report['groups']['kinematics']['status'], 'identical')

    def test_different_types_prevent_pairing_later_ids(self):
        left = transcript(point(0))
        right = transcript(point(0, members=(('1', 'minecraft:wolf'), COW),
                                 changes={'2': {'health': '40a00000'}}))
        report = self.compare(left, right)
        self.assertEqual(report['groups']['ids']['first']['left'], [PIG, COW])
        self.assertEqual(report['groups']['health']['status'], 'identical')

    def test_membership_order_independent_of_ids_turns_and_state(self):
        left = transcript(point(1, turns=(PIG, COW)))
        right = transcript(point(1, members=(COW, PIG), turns=(PIG, COW)))
        report = self.compare(left, right)
        self.assertEqual(report['earliest']['group'], 'membership_order')
        self.assertEqual([name for name, group in report['groups'].items() if group['first']], ['membership_order'])

    def test_turn_order_independent_of_membership_and_state(self):
        left = transcript(point(1, turns=(PIG, COW)))
        right = transcript(point(1, turns=(COW, PIG)))
        report = self.compare(left, right)
        self.assertEqual(report['earliest']['group'], 'turns')
        self.assertEqual([name for name, group in report['groups'].items() if group['first']], ['turns'])

    def test_departure_on_one_side_only(self):
        left = transcript(point(1, turns=(PIG, COW), departures=[(PIG, 'killed')]))
        right = transcript(point(1, turns=(PIG, COW)))
        report = self.compare(left, right)
        self.assertEqual(report['earliest']['group'], 'departures')
        self.assertEqual(report['earliest']['entity'], PIG)
        self.assertEqual(report['groups']['turns']['status'], 'identical')

    def test_first_changed_field_and_differing_point_count(self):
        unchanged = [point(i) for i in range(5)]
        changed = unchanged[:2] + [point(i, changes={'2': {'health': '40a00000'}}) for i in range(2, 5)]
        report = self.compare(transcript(*unchanged), transcript(*changed))
        self.assertEqual(report['groups']['health']['differing_points'], 3)
        first = report['earliest']
        self.assertEqual((first['step'], first['entity'], first['field']), (2, COW, 'health'))
        self.assertEqual((first['left'], first['right']), ('41200000', '40a00000'))
        self.assertTrue(any('health=40a00000' in row['text'] for row in first['right_context']))
        self.assertIn('Later differences', first['note'])

    def test_unknown_on_one_side(self):
        left = transcript(point(0))
        right = transcript(point(0, changes={'2': {'random': '?unmodeled'}}))
        report = self.compare(left, right)
        self.assertEqual(report['groups']['entity_random']['unknown_values'], 1)
        self.assertEqual(report['earliest']['entity'], COW)
        self.assertEqual(self.cli(left, right)[0], 1)

    def test_identical_unknowns_are_differences(self):
        data = transcript(point(0, changes={'2': {'random': '?unmodeled'}}))
        report = self.compare(data, data)
        self.assertEqual(report['groups']['entity_random']['unknown_values'], 2)
        self.assertEqual(report['groups']['entity_random']['differing_points'], 1)
        self.assertEqual(self.cli(data, data)[0], 1)

    def test_truncated_file_rejected(self):
        data = transcript(point(0))
        code, output = self.cli(data, data[:data.index('Z\t')])
        self.assertEqual(code, 2)
        self.assertIn(f'{self.right}:6: expected Z record', output)

    def test_missing_point_with_a_valid_footer_is_rejected(self):
        code, output = self.cli(transcript(point(0), point(1)), transcript(point(0)))
        self.assertEqual(code, 2)
        self.assertIn(f'{self.right}:6: point None', output)

    def test_unknown_departure_reason_counts_on_each_side(self):
        data = transcript(point(1, departures=[(PIG, '?')]))
        report = self.compare(data, data)
        self.assertEqual(report['groups']['departures']['unknown_values'], 2)
        self.assertEqual(report['earliest']['field'], 'reason')

    def test_header_mismatch_rejected(self):
        data = transcript(point(0))
        for field, original, replacement in [
            ('script_sha256', 'a' * 64, 'c' * 64), ('fixture_sha256', 'b' * 64, 'd' * 64),
            ('version', '1', '2'), ('game_version', '26.3', '26.2'),
        ]:
            with self.subTest(field=field):
                code, output = self.cli(data, data.replace(f'\t{field}={original}', f'\t{field}={replacement}'))
                self.assertEqual(code, 2)
                self.assertIn(f'{self.right}:1:', output)
                self.assertIn(field, output)

    def test_ignored_group_leaves_other_differences_and_json(self):
        left = transcript(point(1, turns=(PIG, COW)))
        right = transcript(point(1, turns=(COW, PIG), changes={'2': {'health': '40a00000'}}))
        json_path = Path(self.directory.name) / 'report.json'
        code, output = self.cli(left, right, '--ignore-group', 'turns', '--ignore-group', 'uuid',
                                '--json', str(json_path))
        self.assertEqual(code, 1)
        self.assertIn('turns: ignored', output)
        self.assertIn('uuid: ignored', output)
        self.assertIn('Earliest difference (health)', output)
        report = json.loads(json_path.read_text(encoding='utf-8'))
        self.assertEqual(report['groups']['turns']['status'], 'ignored')
        self.assertEqual(report['earliest']['group'], 'health')
        self.assertEqual(self.cli(left, right, '--ignore-group', 'turns', '--ignore-group', 'health')[0], 0)

    def test_malformed_records_rejected(self):
        data = transcript(point(0), point(1, turns=(PIG, COW)))
        for original, replacement, line in [
            ('\tengine=sunrise', '\tengine=unknown', 1),
            ('\tord=0', '\tord=1', 2),
            ('\tmembers=2', '\tmembers=3', 3),
            ('\tground=1', '\tground=maybe', 4),
            ('\tpos=0000', '\tpos=ABCD', 4),
            ('\ti=1\tid=2', '\ti=1\tid=1', 5),
            ('\tn=1\tid=2', '\tn=2\tid=2', 11),
            ('\tpoints=2', '\tpoints=1', 12),
            ('\trows=10', '\trows=9', 12),
        ]:
            with self.subTest(original=original):
                code, output = self.cli(data, data.replace(original, replacement, 1))
                self.assertEqual(code, 2)
                self.assertIn(f'{self.right}:{line}:', output)

    def test_departure_record_on_after_op_rejected(self):
        data = transcript(point(0, departures=[(PIG, 'killed')]))
        code, output = self.cli(data, data)
        self.assertEqual(code, 2)
        self.assertIn(f'{self.left}:6:', output)


class StateTranscriptSceneTests(unittest.TestCase):
    def test_baseline_and_vocabulary(self):
        command_prefix = [
            '/difficulty peaceful', '/time set night', '/gamerule spawn_mobs false',
            '/gamerule spawn_wandering_traders false', '/gamerule advance_weather false',
            '/gamerule random_tick_speed 0', '/tp @s', '/gamemode creative',
        ]
        for name in scenes.SCENES:
            with self.subTest(scene=name):
                script = scenes.scene_script(name)
                self.assertTrue(script.endswith('\n'))
                ops = [json.loads(line) for line in script.splitlines()]
                allowed = {'cmd', 'settle', 'wait_chunks', 'align_game_time', 'sync_randoms', 'state_transcript', 'tick'}
                self.assertLessEqual({op['op'] for op in ops}, allowed)
                self.assertEqual(
                    [op['op'] for op in ops[:16]],
                    ['cmd'] * 8 + ['settle', 'cmd', 'wait_chunks', 'settle', 'align_game_time',
                                   'cmd', 'sync_randoms', 'state_transcript'],
                )
                for op, prefix in zip(ops, command_prefix):
                    self.assertTrue(op['cmd'].startswith(prefix))
                self.assertTrue(ops[9]['cmd'].startswith('/forceload add '))
                self.assertEqual(ops[10]['chunks'], ops[11]['required_chunks'])
                self.assertEqual(ops[13]['cmd'], '/difficulty hard')
                self.assertEqual(set(ops[14]), {'op', 'uniquifier', 'entity_id', 'level_seed',
                                                'rand_value', 'player_seed'})
                self.assertIs(ops[15]['on'], True)
                self.assertEqual([op['n'] for op in ops if op['op'] == 'tick'], [1] * 200)
                self.assertLessEqual({op['op'] for op in ops[16:]}, {'cmd', 'tick'})

    def test_base_summons_every_species(self):
        ops = list(scenes.scene_ops('base'))
        kinds = Counter(op['cmd'].split()[1] for op in ops if op.get('cmd', '').startswith('/summon '))
        for species in ('pig', 'cow', 'sheep', 'chicken', 'rabbit', 'zombie', 'skeleton', 'spider', 'wolf'):
            self.assertEqual(kinds[f'minecraft:{species}'], 20)
        self.assertNotIn('minecraft:villager', kinds)
        self.assertIn('minecraft:tnt', kinds)
        self.assertIn('minecraft:item', kinds)


if __name__ == '__main__':
    unittest.main()
