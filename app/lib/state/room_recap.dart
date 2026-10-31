import 'dart:convert';
import 'dart:io';

import '../protocol/models.dart';

/// What the owner last saw of a room: when, and each task's state.
///
/// From the live room: after a few hours away the owner had to scroll back
/// through hundreds of messages to learn what finished and what was waiting
/// on them. The room keeps running while the app is closed, so comparing
/// what is there now with this record tells them in one card.
class RoomSeen {
  const RoomSeen({required this.at, required this.tasks});

  final DateTime at;
  final Map<String, TaskState> tasks;

  factory RoomSeen.now(List<Task> tasks, {DateTime? at}) => RoomSeen(
    at: at ?? DateTime.now(),
    tasks: {for (final task in tasks) task.id: task.state},
  );

  Map<String, Object?> toJson() => {
    'at': at.millisecondsSinceEpoch,
    'tasks': {for (final e in tasks.entries) e.key: e.value.name},
  };

  static RoomSeen? fromJson(Object? json) {
    if (json is! Map) return null;
    final at = json['at'];
    final tasks = json['tasks'];
    if (at is! int || tasks is! Map) return null;
    return RoomSeen(
      at: DateTime.fromMillisecondsSinceEpoch(at),
      tasks: {
        for (final e in tasks.entries)
          if (e.key is String && e.value is String)
            e.key as String: TaskState.values.firstWhere(
              (state) => state.name == e.value,
              orElse: () => TaskState.unknown,
            ),
      },
    );
  }

  static File file(Directory archive) => File('${archive.path}/seen.json');

  static Future<RoomSeen?> load(Directory archive) async {
    try {
      return fromJson(jsonDecode(await file(archive).readAsString()));
    } on FileSystemException {
      return null;
    } on FormatException {
      return null;
    }
  }

  Future<void> save(Directory archive) async {
    final target = file(archive);
    final temp = File('${target.path}.tmp');
    await temp.writeAsString(jsonEncode(toJson()), flush: true);
    await temp.rename(target.path);
  }
}

class RoomRecap {
  const RoomRecap({
    required this.away,
    required this.messages,
    required this.allMessages,
    required this.busiest,
    required this.finished,
    required this.added,
    required this.stuck,
    required this.waiting,
    required this.removals,
  });

  final Duration away;
  final int messages;

  /// Whether every message the app holds is newer, so there may be more.
  final bool allMessages;

  /// Who wrote the most, most first, at most three.
  final List<(String, int)> busiest;
  final List<String> finished;
  final List<String> added;

  /// Tasks that became blocked or failed.
  final List<String> stuck;

  /// Decisions waiting for the owner now.
  final int waiting;

  /// File removals waiting for the owner's review now.
  final int removals;

  List<String> get lines => [
    if (messages > 0)
      '${allMessages ? 'At least ' : ''}$messages '
          '${messages == 1 ? 'message' : 'messages'}'
          '${busiest.isEmpty ? '' : ', most from ${busiest.map((b) => '${b.$1} (${b.$2})').join(', ')}'}.',
    if (finished.isNotEmpty) 'Done: ${_titles(finished)}.',
    if (added.isNotEmpty) 'New: ${_titles(added)}.',
    if (stuck.isNotEmpty) 'Blocked or failed: ${_titles(stuck)}.',
    if (waiting > 0)
      waiting == 1
          ? '1 decision is waiting for you.'
          : '$waiting decisions are waiting for you.',
    if (removals > 0)
      removals == 1
          ? '1 file removal is waiting for your review.'
          : '$removals file removals are waiting for your review.',
  ];

  String get heading => 'Since you left ${awayText(away)} ago';

  static String _titles(List<String> titles) => titles.length <= 3
      ? titles.map((t) => '"$t"').join(', ')
      : '${titles.take(3).map((t) => '"$t"').join(', ')} and ${titles.length - 3} more';
}

String awayText(Duration away) {
  if (away.inDays >= 1) {
    return away.inDays == 1 ? '1 day' : '${away.inDays} days';
  }
  if (away.inHours >= 1) {
    return away.inHours == 1 ? '1 hour' : '${away.inHours} hours';
  }
  return '${away.inMinutes} minutes';
}

const _done = {TaskState.approved, TaskState.submitted};
const _stuck = {TaskState.blocked, TaskState.failed};

/// What changed since [seen], or null when the owner was barely away or
/// nothing happened.
RoomRecap? recapSince(
  RoomSeen seen, {
  required List<Task> tasks,
  required List<ChatMessage> messages,
  required RoomDigest digest,
  required String Function(Author author) nameOf,
  DateTime? now,
  Duration minimum = const Duration(minutes: 10),
}) {
  final away = (now ?? DateTime.now()).difference(seen.at);
  if (away < minimum) return null;
  final since = seen.at.millisecondsSinceEpoch;
  final newer = [
    for (final message in messages)
      if (message.sentAt.millis > since && !message.author.isHuman) message,
  ];
  final counts = <String, int>{};
  for (final message in newer) {
    final name = nameOf(message.author);
    counts[name] = (counts[name] ?? 0) + 1;
  }
  final busiest = counts.entries.toList()
    ..sort((a, b) => b.value.compareTo(a.value));
  final finished = <String>[];
  final added = <String>[];
  final stuck = <String>[];
  for (final task in tasks) {
    final before = seen.tasks[task.id];
    if (before == null) {
      added.add(task.title);
    }
    if (_done.contains(task.state) && !_done.contains(before)) {
      finished.add(task.title);
    } else if (_stuck.contains(task.state) && before != task.state) {
      stuck.add(task.title);
    }
  }
  final recap = RoomRecap(
    away: away,
    messages: newer.length,
    allMessages:
        newer.isNotEmpty &&
        messages.isNotEmpty &&
        messages.first.sentAt.millis > since,
    busiest: [for (final e in busiest.take(3)) (e.key, e.value)],
    finished: finished,
    added: added,
    stuck: stuck,
    waiting: digest.needsYou.length,
    removals: digest.pendingRemovalTotal,
  );
  return recap.lines.isEmpty ? null : recap;
}
