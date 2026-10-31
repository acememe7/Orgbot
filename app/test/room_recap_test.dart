import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/state/room_recap.dart';

Task _task(String id, String title, String state) =>
    Task.fromJson({'id': id, 'title': title, 'state': state});

ChatMessage _message(
  String id,
  String agent,
  DateTime at, {
  String kind = 'agent',
}) => ChatMessage(
  id: id,
  author: Author(kind: kind, agentId: agent, name: agent),
  body: 'hi',
  intent: 'chat',
  taskId: null,
  sentAt: Timestamp(at.millisecondsSinceEpoch),
);

void main() {
  final left = DateTime(2026, 10, 31, 12);
  final now = DateTime(2026, 10, 31, 15, 20);
  final seen = RoomSeen(
    at: left,
    tasks: {
      'a': TaskState.inProgress,
      'b': TaskState.ready,
      'c': TaskState.approved,
    },
  );

  test('names what finished, what is new, what got stuck and who talked', () {
    final recap = recapSince(
      seen,
      tasks: [
        _task('a', 'Fix the stage lights', 'approved'),
        _task('b', 'Write the march route', 'blocked'),
        _task('c', 'Already done', 'approved'),
        _task('d', 'Print the flyers', 'ready'),
      ],
      messages: [
        _message('m0', 'sol', left.subtract(const Duration(minutes: 5))),
        _message('m1', 'sol', left.add(const Duration(minutes: 1))),
        _message('m2', 'sol', left.add(const Duration(minutes: 2))),
        _message('m3', 'ada', left.add(const Duration(minutes: 3))),
        _message(
          'm4',
          'owner',
          left.add(const Duration(minutes: 4)),
          kind: 'human',
        ),
      ],
      digest: RoomDigest.empty(),
      nameOf: (author) => '@${author.name}',
      now: now,
    )!;
    expect(recap.heading, 'Since you left 3 hours ago');
    expect(recap.lines, [
      '3 messages, most from @sol (2), @ada (1).',
      'Done: "Fix the stage lights".',
      'New: "Print the flyers".',
      'Blocked or failed: "Write the march route".',
    ]);
  });

  test('a short absence or a quiet room shows nothing', () {
    final quiet = [
      _task('a', 'x', 'in_progress'),
      _task('b', 'y', 'ready'),
      _task('c', 'z', 'approved'),
    ];
    expect(
      recapSince(
        seen,
        tasks: quiet,
        messages: const [],
        digest: RoomDigest.empty(),
        nameOf: (_) => '',
        now: left.add(const Duration(minutes: 5)),
      ),
      isNull,
    );
    expect(
      recapSince(
        seen,
        tasks: quiet,
        messages: const [],
        digest: RoomDigest.empty(),
        nameOf: (_) => '',
        now: now,
      ),
      isNull,
    );
  });

  test('says there may be more when every held message is newer', () {
    final recap = recapSince(
      seen,
      tasks: const [],
      messages: [_message('m1', 'sol', left.add(const Duration(minutes: 1)))],
      digest: RoomDigest.empty(),
      nameOf: (_) => '@sol',
      now: now,
    )!;
    expect(recap.lines.first, 'At least 1 message, most from @sol (1).');
  });

  test('the record survives a save and load', () async {
    final dir = await Directory.systemTemp.createTemp('recap');
    addTearDown(() => dir.delete(recursive: true));
    await seen.save(dir);
    final loaded = (await RoomSeen.load(dir))!;
    expect(loaded.at, left);
    expect(loaded.tasks, seen.tasks);
    await RoomSeen.file(dir).writeAsString('{not json');
    expect(await RoomSeen.load(dir), isNull);
  });
}
