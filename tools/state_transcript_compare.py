#!/usr/bin/env python3
"""Validate and compare version 1 Sunrise state transcripts."""

import argparse
from dataclasses import dataclass, field
from itertools import zip_longest
import json
from pathlib import Path
import re
import sys


FORMAT = 'sunrise-state-transcript'
UINT = r'0|[1-9][0-9]{0,19}'
INT = rf'{UINT}|-[1-9][0-9]{{0,18}}'
SHA256 = r'[0-9a-f]{64}'
KEY = r'[a-z0-9_.-]+:[a-z0-9_./-]+'
BIT = r'[01]'
F32 = r'[0-9a-f]{8}'
F64 = r'[0-9a-f]{16}'
RANDOM = rf'[0-9a-f]{{12}},(?:{F64}|-)'
UUID = r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}'
UNKNOWN = r'\?[a-zA-Z0-9_.$:/+-]+'
HEADER = dict(format=FORMAT, version='1', engine='vanilla|sunrise', script_sha256=SHA256,
              fixture_sha256=SHA256, build=r'[\x21-\x7e]+', game_version=r'26\.3')
POINT = dict(step=UINT, phase='after_op|after_tick', ord=UINT, op=r'[1-9][0-9]{0,19}',
             game_time=INT, dim='minecraft:overworld')
LEVEL = dict(level_random=RANDOM, rand_value=INT, last_entity_id=INT)
MEMBER = dict(
    pos=rf'{F64},{F64},{F64}', motion=rf'{F64},{F64},{F64}', rot=rf'{F32},{F32}', ground=BIT,
    tick_count=INT, fall_distance=F64, fire=INT, air=INT, invulnerable_time=INT,
    no_gravity=BIT, vehicle=rf'{INT}|-', passengers=rf'(?:{INT})(?:,(?:{INT}))*|-',
    random=RANDOM, health=rf'{F32}|-', uuid=UUID,
)
IDENTITY = dict(id=INT, type=KEY)
TURN = dict(role='root|passenger', vehicle=MEMBER['vehicle'])
DEPARTURE = dict(reason=r'[a-z][a-z0-9_]*|\?')
MEMBER_GROUPS = {
    'kinematics': ('pos', 'motion', 'rot', 'ground'),
    'base_state': ('fall_distance', 'fire', 'air', 'invulnerable_time', 'no_gravity'),
    'riding': ('vehicle', 'passengers'),
    'entity_random': ('random',),
    'health': ('health',),
    'tick_count': ('tick_count',),
    'uuid': ('uuid',),
}
GROUPS = ('ids', 'membership_order', 'turns', 'departures',
          *[group for group in MEMBER_GROUPS if group != 'uuid'], 'level', 'uuid')


class InvalidTranscript(ValueError):
    pass


@dataclass
class Row:
    line: int
    fields: dict

    @property
    def identity(self):
        return self.fields['id'], self.fields['type']


@dataclass
class Point:
    row: Row
    level: Row
    members: list[Row] = field(default_factory=list)
    turns: list[Row] = field(default_factory=list)
    departures: list[Row] = field(default_factory=list)

    @property
    def key(self):
        fields = self.row.fields
        return int(fields['step']), int(fields['ord']), fields['phase']


@dataclass
class Transcript:
    path: Path
    lines: list[str]
    header: Row
    points: list[Point]
    footer: Row

    def context(self, line):
        start = max(0, line - 3)
        return [{'line': i + 1, 'text': text} for i, text in
                enumerate(self.lines[start:line + 2], start)]


class Reader:
    def __init__(self, path):
        self.path = path
        try:
            raw_lines = path.read_bytes().split(b'\n')
        except OSError as error:
            raise InvalidTranscript(f'{path}:1: {error.strerror}') from error
        if raw_lines[-1] == b'':
            raw_lines.pop()
        self.lines = []
        for number, raw in enumerate(raw_lines, 1):
            try:
                self.lines.append(raw.decode('utf-8'))
            except UnicodeDecodeError:
                self.fail('invalid UTF-8', number)
        self.index = 0

    def fail(self, message, line=None):
        raise InvalidTranscript(f'{self.path}:{line or self.index + 1}: {message}')

    def next_kind(self):
        if self.index == len(self.lines):
            return None
        return self.lines[self.index].partition('\t')[0]

    def read(self, kind, schema, unknown=()):
        number = self.index + 1
        if self.next_kind() != kind:
            self.fail(f'expected {kind} record')
        parts = self.lines[self.index].split('\t')[1:]
        if len(parts) != len(schema):
            self.fail(f'{kind} record requires fields: {", ".join(schema)}')
        fields = {}
        for part, (name, pattern) in zip(parts, schema.items()):
            prefix, _, value = part.partition('=')
            if prefix != name:
                self.fail(f'expected {kind}.{name}, got {part!r}')
            if not re.fullmatch(pattern, value):
                if name not in unknown or not re.fullmatch(UNKNOWN, value):
                    self.fail(f'invalid {kind}.{name}: {value!r}')
            fields[name] = value
        self.index += 1
        return Row(number, fields)

    def stream(self, kind, index, schema, unknown=()):
        seen = set()
        rows = []
        while self.next_kind() == kind:
            row = self.read(kind, {index: UINT, **IDENTITY, **schema}, unknown)
            if row.fields[index] != str(len(rows)):
                self.fail(f'{kind}.{index} must be {len(rows)}', row.line)
            entity_id = row.fields['id']
            if entity_id in seen:
                self.fail(f'duplicate {kind} id {entity_id}', row.line)
            seen.add(entity_id)
            rows.append(row)
        return rows

    def point(self, previous):
        row = self.read('P', POINT)
        step, ordinal, op = (int(row.fields[key]) for key in ('step', 'ord', 'op'))
        tick = row.fields['phase'] == 'after_tick'
        previous_step, previous_ord = previous.key[:2] if previous else (0, -1)
        expected_step = previous_step + tick
        expected_ord = 0 if tick else previous_ord + 1
        if (step, ordinal) != (expected_step, expected_ord):
            self.fail(f'expected step={expected_step} ord={expected_ord}', row.line)
        if previous:
            previous_op = int(previous.row.fields['op'])
            repeated_tick = tick and previous.row.fields['phase'] == 'after_tick'
            if op < previous_op or (op == previous_op and not repeated_tick):
                self.fail('op must advance to the next script line or continue a multi-tick op', row.line)
        level = self.read('L', {**LEVEL, 'members': UINT}, LEVEL)
        members = self.stream('M', 'i', MEMBER, MEMBER)
        if level.fields['members'] != str(len(members)):
            self.fail(f'L.members declares {level.fields["members"]}, found {len(members)} M records', level.line)
        point = Point(row, level, members)
        if tick:
            point.turns = self.stream('T', 'n', TURN, ('vehicle',))
            point.departures = self.stream('D', 'n', DEPARTURE, DEPARTURE)
        return point

    def parse(self):
        header = self.read('H', HEADER)
        points = []
        while self.next_kind() == 'P':
            points.append(self.point(points[-1] if points else None))
        rows = self.index - 1
        footer = self.read('Z', dict(points=UINT, rows=UINT))
        for name, actual in [('points', len(points)), ('rows', rows)]:
            if footer.fields[name] != str(actual):
                self.fail(f'Z.{name} declares {footer.fields[name]}, found {actual}', footer.line)
        if self.index != len(self.lines):
            self.fail('unexpected record after Z')
        return Transcript(self.path, self.lines, header, points, footer)


def read_transcript(path):
    return Reader(Path(path)).parse()


@dataclass
class Difference:
    field: str
    left: object
    right: object
    left_line: int
    right_line: int
    entity: tuple | None = None

    def report(self, key):
        return {
            'step': key[0], 'phase': key[2], 'ord': key[1],
            'entity': self.entity, 'field': self.field, 'left': self.left, 'right': self.right,
            'left_line': self.left_line, 'right_line': self.right_line,
        }


def differs(left, right):
    return left != right or left.startswith('?') or right.startswith('?')


def field_differences(left, right, fields, entity=None):
    for name in fields:
        a, b = left.fields[name], right.fields[name]
        if differs(a, b):
            yield Difference(name, a, b, left.line, right.line, entity)


def sequence_difference(left, right, left_point, right_point, name, fields=()):
    for index, (a, b) in enumerate(zip_longest(left, right)):
        left_id = a.identity if a else None
        right_id = b.identity if b else None
        if left_id != right_id:
            return Difference(f'{name}[{index}]', left_id, right_id,
                              a.line if a else left_point.line, b.line if b else right_point.line,
                              left_id or right_id)
        if fields and (difference := next(field_differences(a, b, fields, left_id), None)):
            return difference
    return None


def compare_ids(left, right):
    ordered_left = sorted(left.members, key=lambda row: int(row.fields['id']))
    ordered_right = sorted(right.members, key=lambda row: int(row.fields['id']))
    matched = []
    for a, b in zip(ordered_left, ordered_right):
        if a.identity != b.identity:
            break
        matched.append((a, b))
    suffix_left = ordered_left[len(matched):]
    suffix_right = ordered_right[len(matched):]
    if not (suffix_left or suffix_right):
        return matched, None
    a = suffix_left[0] if suffix_left else None
    b = suffix_right[0] if suffix_right else None
    difference = Difference('unmatched', [row.identity for row in suffix_left],
                            [row.identity for row in suffix_right],
                            a.line if a else left.row.line, b.line if b else right.row.line,
                            (a or b).identity)
    return matched, difference


def unknown_at_point(group, point):
    if group in MEMBER_GROUPS:
        rows, fields = point.members, MEMBER_GROUPS[group]
    else:
        rows, fields = {
            'turns': (point.turns, TURN),
            'departures': (point.departures, DEPARTURE),
            'level': ([point.level], LEVEL),
        }.get(group, ((), ()))
    return [(row, name) for row in rows for name in fields if row.fields[name].startswith('?')]


def point_differences(left, right):
    matched, id_difference = compare_ids(left, right)
    differences = {
        'ids': id_difference,
        'membership_order': sequence_difference(left.members, right.members, left.row, right.row, 'member'),
        'turns': sequence_difference(left.turns, right.turns, left.row, right.row, 'turn', TURN),
        'departures': sequence_difference(left.departures, right.departures, left.row, right.row, 'departure',
                                          DEPARTURE),
        'level': next(field_differences(left.row, right.row, ('game_time', 'dim')), None)
        or next(field_differences(left.level, right.level, LEVEL), None),
    }
    for group, fields in MEMBER_GROUPS.items():
        differences[group] = next(
            (difference for a, b in matched for difference in field_differences(a, b, fields, a.identity)),
            None,
        )
    return differences


def compare(left, right, ignored=()):
    for name in ('script_sha256', 'fixture_sha256'):
        a, b = left.header.fields[name], right.header.fields[name]
        if a != b:
            raise InvalidTranscript(f'{right.path}:1: {name} {b} does not match {left.path}:1 ({a})')

    groups = {name: {'ignored': name in ignored, 'differing_points': 0, 'unknown_values': 0, 'first': None}
              for name in GROUPS}
    earliest = None
    for a, b in zip_longest(left.points, right.points):
        a_line = a.row.line if a else left.footer.line
        b_line = b.row.line if b else right.footer.line
        a_key = (*a.key, a.row.fields['op']) if a else None
        b_key = (*b.key, b.row.fields['op']) if b else None
        if a_key != b_key:
            raise InvalidTranscript(f'{right.path}:{b_line}: point {b_key} differs from '
                                    f'{left.path}:{a_line} ({a_key}) for the same script')
        differences = point_differences(a, b)
        for name, group in groups.items():
            unknown_left = unknown_at_point(name, a)
            unknown_right = unknown_at_point(name, b)
            group['unknown_values'] += len(unknown_left) + len(unknown_right)
            difference = differences.get(name)
            if difference is None and (unknown_left or unknown_right):
                row, field_name = (unknown_left or unknown_right)[0]
                value = row.fields[field_name]
                entity = row.identity if 'id' in row.fields else None
                difference = (Difference(field_name, value, None, row.line, b_line, entity) if unknown_left
                              else Difference(field_name, None, value, a_line, row.line, entity))
            if difference:
                group['differing_points'] += 1
                if group['first'] is None:
                    group['first'] = difference.report(a.key)
                if earliest is None and not group['ignored']:
                    earliest = {
                        'group': name, **difference.report(a.key),
                        'left_context': left.context(difference.left_line),
                        'right_context': right.context(difference.right_line),
                        'note': 'Later differences may be consequences of this one.',
                    }
    for group in groups.values():
        group['status'] = ('ignored' if group['ignored'] else 'different' if group['first'] else 'identical')
    return {'left': str(left.path), 'right': str(right.path), 'groups': groups, 'earliest': earliest}


def describe(difference):
    point = f'step {difference["step"]} {difference["phase"]} ord {difference["ord"]}'
    entity = difference['entity']
    subject = f' entity {entity[0]} ({entity[1]})' if entity else ''
    return f'{point}{subject} {difference["field"]}: {difference["left"]!r} vs {difference["right"]!r}'


def print_report(report):
    print(f'{report["left"]} versus {report["right"]}')
    for name, group in report['groups'].items():
        print(f'{name}: {group["status"]} ({group["differing_points"]} differing points, '
              f'{group["unknown_values"]} unknown values)')
        if group['first']:
            print(f'  {describe(group["first"])}')
    first = report['earliest']
    if first:
        print(f'\nEarliest difference ({first["group"]}): {describe(first)}')
        for side in ('left', 'right'):
            print(f'  {report[side]}:')
            for line in first[f'{side}_context']:
                print(f'    {line["line"]}: {line["text"]}')
        print(first['note'])


def report_comparison(left, right, ignored=(), json_path=None):
    report = compare(left, right, ignored)
    print_report(report)
    if json_path is not None:
        Path(json_path).write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    return int(report['earliest'] is not None)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('left', type=Path)
    parser.add_argument('right', type=Path)
    parser.add_argument('--ignore-group', choices=GROUPS, action='append', default=[])
    parser.add_argument('--json', type=Path, metavar='PATH')
    args = parser.parse_args(argv)
    try:
        return report_comparison(read_transcript(args.left), read_transcript(args.right),
                                 args.ignore_group, args.json)
    except (InvalidTranscript, OSError) as error:
        print(error, file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
