/// A room's saved data on this device, and deleting it.
library;

import 'dart:io';

import '../engine/core_server.dart' show linuxServiceDirectory;
import 'room_archive.dart';

/// The folders this app keeps for one room: its history (board, transcript,
/// deliverables, CLI sessions and the bots' working copies) and, on Linux,
/// its background service's folder. Never the project folder.
Future<List<String>> roomDataFolders(String roomId) async => [
  (await RoomArchive.directory(roomId)).path,
  if (Platform.isLinux) linuxServiceDirectory(roomId),
];

/// How many bytes a room's saved data takes, or null where that cannot be
/// measured cheaply.
Future<int?> roomDataBytes(String roomId) async {
  if (!Platform.isLinux) return null;
  final folders = <String>[
    for (final folder in await roomDataFolders(roomId))
      if (await FileSystemEntity.type(folder, followLinks: false) == FileSystemEntityType.directory) folder,
  ];
  if (folders.isEmpty) return 0;
  try {
    final result = await Process.run('du', ['-s', '-B1', '--', ...folders]);
    if (result.exitCode != 0 && (result.stdout as String).trim().isEmpty) return null;
    var total = 0;
    for (final line in (result.stdout as String).split('\n')) {
      total += int.tryParse(line.split('\t').first.trim()) ?? 0;
    }
    return total;
  } on ProcessException {
    return null;
  }
}

/// Delete a room's saved data. The project folder is never touched: if one of
/// `keep` lies inside a folder about to go, nothing is deleted and this throws.
///
/// From 13 November 2026: the owner deleted finished rooms in the app, their
/// history and working copies stayed (165 GB), and they removed them by hand.
Future<void> deleteRoomData(String roomId, {List<String> keep = const []}) =>
    deleteFolders(roomDataFolders(roomId), keep: keep);

/// Delete each folder in `folders`, without following links, after checking
/// none of `keep` is inside one of them. Read-only folders in the bots'
/// copies are made writable first.
Future<void> deleteFolders(
  Future<List<String>> folders, {
  List<String> keep = const [],
}) async {
  final targets = <String>[];
  for (final folder in await folders) {
    final type = await FileSystemEntity.type(folder, followLinks: false);
    if (type == FileSystemEntityType.notFound) continue;
    if (type != FileSystemEntityType.directory) {
      throw FileSystemException('Not deleted: this is not a plain folder', folder);
    }
    targets.add(await Directory(folder).resolveSymbolicLinks());
  }
  for (final raw in keep.where((path) => path.trim().isNotEmpty)) {
    final path = await _resolved(raw.trim());
    for (final target in targets) {
      if (path == target || path.startsWith('$target/')) {
        throw FileSystemException(
          'Nothing was deleted: the project folder is inside the room\'s saved data',
          path,
        );
      }
    }
  }
  for (final target in targets) {
    try {
      await Directory(target).delete(recursive: true);
    } on FileSystemException {
      if (!Platform.isLinux && !Platform.isAndroid) rethrow;
      // Copies keep the project's folder modes, read-only ones included.
      // chmod -R does not follow links found inside.
      await Process.run('chmod', ['-R', 'u+rwX', '--', target]);
      await Directory(target).delete(recursive: true);
    }
  }
}

Future<String> _resolved(String path) async {
  final expanded = path.startsWith('~/')
      ? '${Platform.environment['HOME'] ?? ''}${path.substring(1)}'
      : path;
  try {
    return await Directory(expanded).resolveSymbolicLinks();
  } on FileSystemException {
    return Directory(expanded).absolute.path;
  }
}
