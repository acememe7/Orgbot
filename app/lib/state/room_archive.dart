import 'dart:async';
import 'dart:convert';
import 'dart:io';
import '../workspace/workspace.dart';

/// Read-only run history. Reopening a log never re-executes model/tool calls.
class RoomArchive {
  final File file;
  final void Function(String) onError;
  Future<void> _writing = Future.value();
  int _bytes = 0;
  bool _failed = false;
  RoomArchive._(this.file, this.onError);

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
    void Function(String) onError,
  ) async {
    final dir = await directory(roomId);
    await dir.create(recursive: true);
    final file = File(
      '${dir.path}/${DateTime.now().microsecondsSinceEpoch}.jsonl',
    );
    await file.create(exclusive: true);
    return RoomArchive._(file, onError);
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
    _bytes += utf8.encode(text).length;
    // ponytail: cap a run log at 20 MiB; segmented logs only if real runs reach this.
    if (_bytes > 20 * 1024 * 1024) {
      _failed = true;
      onError('Run history reached 20 MiB. This run is no longer being saved.');
      return;
    }
    _writing = _writing
        .then((_) async {
          await file.writeAsString(text, mode: FileMode.append, flush: true);
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
