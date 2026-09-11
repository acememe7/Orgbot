import 'dart:async';
import 'dart:convert';
import 'dart:io';
import '../workspace/workspace.dart';

/// Read-only run history. Reopening a log never re-executes model/tool calls.
class RoomArchive {
  File file;
  final String roomId;
  final void Function(String) onError;
  Future<void> _writing = Future.value();
  int _bytes = 0;
  bool _failed = false;
  RoomArchive._(this.file, this.roomId, this.onError, this._segmentBytes);

  /// Where this log rolls. A field so a test can reach the boundary without
  /// writing twenty megabytes to find out what happens at it.
  final int _segmentBytes;

  /// The most one segment of a run log may hold before a new one is started.
  ///
  /// A cap is right: one unbounded file is slow to open and impossible to
  /// prune. Stopping at the cap was not — a four-day run reached it and the
  /// transcript simply ended, which is the one thing a history is for. The
  /// note left here said segmented logs could wait until a real run reached
  /// twenty megabytes. One has.
  static const int maxSegmentBytes = 20 * 1024 * 1024;

  /// Segments already written for this run, newest last.
  final List<File> segments = [];

  /// Migrate an older journal once. The original log is retained untouched.
  static Future<File> stateFile(String roomId) async {
    final dir = await directory(roomId);
    await dir.create(recursive: true);
    final state = File('${dir.path}/state.json');
    if (await state.exists()) return state;
    for (final file in await list(roomId)) {
      Map<String, Object?>? room;
      final events = <int, Map<String, Object?>>{};
      await for (final frame in read(file)) {
        if (frame['type'] == 'snapshot') {
          final next = (frame['room'] as Map?)?.cast<String, Object?>();
          if (room != null && room['id'] != next?['id']) events.clear();
          room = next;
        }
        if (frame['type'] == 'event') {
          final event = (frame['envelope'] as Map).cast<String, Object?>();
          events[event['seq'] as int] = event;
        }
        if (frame['type'] == 'phase' && room != null) {
          room['phase'] = frame['phase'];
        }
      }
      if (room == null || events.isEmpty) continue;
      final ordered = events.entries.toList()
        ..sort((a, b) => a.key.compareTo(b.key));
      final temporary = File('${state.path}.migrating');
      await temporary.writeAsString(
        jsonEncode({
          'version': 0,
          'room': room,
          'events': ordered.map((entry) => entry.value).toList(),
        }),
        flush: true,
      );
      await temporary.rename(state.path);
      break;
    }
    return state;
  }

  /// The same snapshot/event frames consumed by the live chat, available offline.
  static Future<List<Map<String, Object?>>> savedFrames(String roomId) async {
    final state = await stateFile(roomId);
    if (!await state.exists()) return [];
    if (await state.length() > 64 * 1024 * 1024) {
      throw const FormatException(
        'Room memory exceeds 64 MiB. Saved files were kept.',
      );
    }
    final data = jsonDecode(await state.readAsString()) as Map;
    if (data['version'] == 0) {
      return [
        {'type': 'snapshot', 'room': data['room'], 'roster': [], 'tasks': []},
        for (final event in data['events'] as List)
          {'type': 'event', 'envelope': event},
        {'type': 'phase', 'phase': 'paused'},
      ];
    }
    if (data['version'] != 1) {
      throw const FormatException('Update the app to read this room memory.');
    }
    final engine = data['engine'] as Map;
    return [
      {'type': 'snapshot', 'room': engine['room'], 'roster': [], 'tasks': []},
      for (final event in engine['log'] as List)
        {'type': 'event', 'envelope': event},
      {'type': 'phase', 'phase': 'paused'},
    ];
  }

  /// Every other room's saved board, so a new room can reuse their work.
  ///
  /// The room being opened is left out: a room offered its own work as
  /// something an earlier room did would cheerfully cite itself.
  static Future<List<String>> otherBoards(String exceptRoomId) async {
    try {
      final here = await directory(exceptRoomId);
      final history = here.parent;
      if (!await history.exists()) return const [];
      final boards = <String>[];
      await for (final entry in history.list()) {
        if (entry is! Directory) continue;
        if (entry.path == here.path) continue;
        final state = File('${entry.path}/state.json');
        if (await state.exists()) boards.add(state.path);
      }
      boards.sort();
      return boards;
    } catch (_) {
      // Reuse is a bonus; failing to look must never stop a room opening.
      return const [];
    }
  }

  static Future<Directory> directory(String roomId) async {
    if (!RegExp(r'^[a-zA-Z0-9_-]+$').hasMatch(roomId)) {
      throw const FormatException('Invalid room identifier.');
    }
    final base = Platform.isAndroid
        ? await WorkspaceConnection.phoneBase()
        : '${Platform.environment['XDG_DATA_HOME'] ?? '${Platform.environment['HOME']}/.local/share'}/orgbot';
    return Directory('$base/.history/$roomId');
  }

  static Future<RoomArchive> open(
    String roomId,
    void Function(String) onError, {
    int segmentBytes = maxSegmentBytes,
  }) async {
    final dir = await directory(roomId);
    await dir.create(recursive: true);
    final file = File(
      '${dir.path}/${DateTime.now().microsecondsSinceEpoch}.jsonl',
    );
    await file.create(exclusive: true);
    final archive = RoomArchive._(file, roomId, onError, segmentBytes);
    archive.segments.add(file);
    return archive;
  }

  void append(List<Map<String, Object?>> messages) {
    if (_failed) return;
    final retained = messages.where(
      (message) => [
        'snapshot',
        'event',
        'phase',
        'closed',
        'workspace_activity',
      ].contains(message['type']),
    );
    if (retained.isEmpty) return;
    final text = '${retained.map(jsonEncode).join('\n')}\n';
    final length = utf8.encode(text).length;
    // Start a new segment rather than stopping. The history view already reads
    // every segment a room has, so a rolled log reads as one run; a log that
    // stops reads as a room that went quiet.
    if (_bytes > 0 && _bytes + length > _segmentBytes) {
      final rolled = File(
        '${file.parent.path}/${DateTime.now().microsecondsSinceEpoch}.jsonl',
      );
      file = rolled;
      segments.add(rolled);
      _bytes = 0;
      _writing = _writing
          .then((_) => rolled.create(exclusive: true))
          .catchError((Object _) => rolled);
    }
    _bytes += length;
    final target = file;
    _writing = _writing
        .then((_) async {
          await target.writeAsString(text, mode: FileMode.append, flush: true);
        })
        .catchError((Object _) {
          if (!_failed) {
            onError(
              'Run history could not be saved. Existing history is preserved.',
            );
          }
          _failed = true;
        });
  }

  Future<void> close() => _writing;
  static Future<List<File>> list(String roomId) async {
    final dir = await directory(roomId);
    if (!await dir.exists()) return [];
    final files = await dir
        .list(followLinks: false)
        .where((entry) => entry is File && entry.path.endsWith('.jsonl'))
        .cast<File>()
        .toList();
    files.sort((a, b) => b.path.compareTo(a.path));
    return files;
  }

  static Stream<Map<String, Object?>> read(File file) async* {
    await for (final line
        in file
            .openRead()
            .transform(utf8.decoder)
            .transform(const LineSplitter())) {
      // An interrupted final append may end mid-line. Keep earlier records.
      try {
        final value = jsonDecode(line);
        if (value is Map) yield value.cast<String, Object?>();
      } on FormatException {
        break;
      }
    }
  }
}
