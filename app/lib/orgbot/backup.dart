import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

import 'models.dart';
import '../state/room_archive.dart';
import '../workspace/desktop_files.dart';
import 'room_check.dart' show expandHome;

/// A portable, data-only room bundle. Importing it never starts a room.
class PortableRoomBackup {
  static const maxBundleBytes = 64 * 1024 * 1024;
  static const maxExpandedBytes = 44 * 1024 * 1024;
  static const maxFiles = 512;
  static const maxFileBytes = 32 * 1024 * 1024;
  static const maxProjectFileBytes = 16 * 1024 * 1024;
  static const maxMemoryFileBytes = 16 * 1024 * 1024;
  static const _version = 1;
  static const _executeBits = 0x49;
  static const _privateFileMode = 0x180;
  static const _privateExecutableMode = 0x1c0;
  static const _picker = MethodChannel('orgbot/workspace');

  final RoomDef room;
  final List<BotDef> bots;
  final Map<String, Object?> state;
  final Map<String, List<int>> files;
  final Map<String, int> _projectModes;

  const PortableRoomBackup._(
    this.room,
    this.bots,
    this.state,
    this.files,
    this._projectModes,
  );

  static Future<List<int>> export({
    required RoomDef room,
    required List<BotDef> bots,
    required Map<String, Object?> state,
    required Map<String, List<int>> files,
    Map<String, int> projectFileModes = const {},
    required bool isStopped,
  }) async {
    if (!isStopped) {
      throw StateError('Stop the room before creating a portable backup.');
    }
    if (state.isNotEmpty && !_checkpointIsPaused(state)) {
      throw StateError('Pause or stop the room before creating a backup.');
    }
    final payloads = <String, List<int>>{
      ...files,
      if (state.isNotEmpty) 'room/state.json': utf8.encode(jsonEncode(state)),
    };
    _validatePayloads(payloads);
    for (final entry in projectFileModes.entries) {
      _validatePath(entry.key);
      if (!entry.key.startsWith('project/') ||
          !payloads.containsKey(entry.key) ||
          entry.value < 0) {
        throw const FormatException(
          'Project file mode metadata has no matching file.',
        );
      }
    }
    final safeRoomJson = _withoutCredentials(room.toJson())..['path'] = '';
    (safeRoomJson['workspace'] as Map<String, Object?>)['path'] = '';
    final safeRoom = RoomDef.fromJson(safeRoomJson);
    final safeBots = bots
        .map((bot) => BotDef.fromJson(_withoutCredentials(bot.toJson())))
        .toList();
    final manifest = <String, Object?>{
      'format': 'orgbot-portable-room',
      'version': _version,
      'room': safeRoom.toJson(),
      'bots': safeBots.map((bot) => bot.toJson()).toList(),
      'files': {
        for (final entry in payloads.entries)
          entry.key: {
            if (projectFileModes.containsKey(entry.key))
              'mode': projectFileModes[entry.key]! & _executeBits,
            'sha256': sha256.convert(entry.value).toString(),
            'data': base64Encode(entry.value),
          },
      },
    };
    manifest['manifest_sha256'] = sha256
        .convert(utf8.encode(jsonEncode(manifest)))
        .toString();
    final encoded = utf8.encode(jsonEncode(manifest));
    if (encoded.length > maxBundleBytes) {
      throw const FormatException('The portable backup exceeds 64 MiB.');
    }
    return encoded;
  }

  static Future<Map<String, List<int>>> roomHistoryFiles(String roomId) async {
    final directory = await RoomArchive.directory(roomId);
    final historyRoot = Directory(directory.parent.path);
    final historyType = await FileSystemEntity.type(
      historyRoot.path,
      followLinks: false,
    );
    if (historyType == FileSystemEntityType.link) {
      throw const FormatException(
        'Saved room history cannot be a symbolic link.',
      );
    }
    final result = <String, List<int>>{};
    var total = 0;
    final state = File('${directory.path}/state.json');
    if (await FileSystemEntity.type(state.path, followLinks: false) ==
        FileSystemEntityType.file) {
      if (await state.length() > maxFileBytes) {
        throw const FormatException('Saved room state exceeds 32 MiB.');
      }
      total += await state.length();
      result['room/state.json'] = await _readStable(state);
    }
    for (final file in await RoomArchive.list(roomId)) {
      if (await FileSystemEntity.type(file.path, followLinks: false) !=
          FileSystemEntityType.file) {
        throw const FormatException(
          'A saved history entry is not a regular file.',
        );
      }
      if (await file.length() > maxFileBytes) {
        throw const FormatException('A saved history file exceeds 32 MiB.');
      }
      total += await file.length();
      if (total > maxExpandedBytes || result.length >= maxFiles) {
        throw const FormatException(
          'Saved room history exceeds backup limits.',
        );
      }
      result['room/history/${file.uri.pathSegments.last}'] = await _readStable(
        file,
      );
    }
    final bodies = Directory('${directory.path}/artifact_bodies');
    if (await FileSystemEntity.type(bodies.path, followLinks: false) ==
        FileSystemEntityType.link) {
      throw const FormatException(
        'Saved artifact folders cannot be symbolic links.',
      );
    }
    if (await bodies.exists()) {
      await for (final entity in bodies.list(
        recursive: true,
        followLinks: false,
      )) {
        final type = await FileSystemEntity.type(
          entity.path,
          followLinks: false,
        );
        if (type == FileSystemEntityType.link) {
          throw const FormatException(
            'Saved artifact folders cannot contain symbolic links.',
          );
        }
        if (type != FileSystemEntityType.file) continue;
        final relative = entity.path.substring(bodies.path.length + 1);
        final file = File(entity.path);
        if (await file.length() > maxFileBytes) {
          throw const FormatException('A saved artifact exceeds 32 MiB.');
        }
        total += await file.length();
        if (total > maxExpandedBytes || result.length >= maxFiles) {
          throw const FormatException(
            'Saved room history exceeds backup limits.',
          );
        }
        result['room/artifact_bodies/$relative'] = await _readStable(file);
      }
    }
    return result;
  }

  static Future<Map<String, List<int>>> projectMemoryFiles(
    String projectPath,
  ) async {
    if (projectPath.trim().isEmpty) return const {};
    final canonical = await Directory(
      expandHome(projectPath),
    ).resolveSymbolicLinks();
    final scopeHash = sha256.convert(utf8.encode('project:$canonical'));
    final support = await getApplicationSupportDirectory();
    final root = Directory('${support.path}/project-memory');
    if (await FileSystemEntity.type(root.path, followLinks: false) ==
        FileSystemEntityType.link) {
      throw const FormatException('Project memory cannot be a symbolic link.');
    }
    final file = File('${root.path}/$scopeHash.json');
    if (await FileSystemEntity.type(file.path, followLinks: false) ==
        FileSystemEntityType.notFound) {
      return const {};
    }
    if (await FileSystemEntity.type(file.path, followLinks: false) !=
            FileSystemEntityType.file ||
        await file.length() > maxMemoryFileBytes) {
      throw const FormatException(
        'Project memory is not a bounded regular file.',
      );
    }
    return {'memory/project.json': await _readStable(file)};
  }

  static PortableRoomBackup parse(List<int> bytes) {
    if (bytes.length > maxBundleBytes) {
      throw const FormatException('The portable backup exceeds 64 MiB.');
    }
    final decoded = jsonDecode(utf8.decode(bytes));
    if (decoded is! Map) {
      throw const FormatException('This is not a supported OrgBots backup.');
    }
    final manifestHash = decoded['manifest_sha256'];
    final unsigned = Map<Object?, Object?>.from(decoded)
      ..remove('manifest_sha256');
    if (manifestHash is! String ||
        sha256.convert(utf8.encode(jsonEncode(unsigned))).toString() !=
            manifestHash) {
      throw const FormatException(
        'The backup manifest failed its integrity check.',
      );
    }
    if (decoded['format'] != 'orgbot-portable-room' ||
        decoded['version'] != _version ||
        decoded['room'] is! Map ||
        decoded['bots'] is! List ||
        decoded['files'] is! Map) {
      throw const FormatException('This is not a supported OrgBots backup.');
    }
    if ((decoded['files'] as Map).length > maxFiles) {
      throw const FormatException('The backup has an invalid file count.');
    }
    final payloads = <String, List<int>>{};
    final projectModes = <String, int>{};
    var totalBytes = 0;
    for (final entry in (decoded['files'] as Map).entries) {
      if (entry.key is! String || entry.value is! Map) {
        throw const FormatException('The backup file manifest is invalid.');
      }
      final path = entry.key as String;
      _validatePath(path);
      final item = (entry.value as Map).cast<String, Object?>();
      final data = item['data'];
      final digest = item['sha256'];
      final mode = item['mode'];
      if (data is! String ||
          digest is! String ||
          data.length > maxFileBytes * 2 ||
          (mode != null &&
              (!path.startsWith('project/') ||
                  mode is! int ||
                  mode < 0 ||
                  mode & ~_executeBits != 0))) {
        throw const FormatException('A backup file entry is invalid.');
      }
      final content = base64Decode(data);
      if (content.length > maxFileBytes ||
          sha256.convert(content).toString() != digest) {
        throw const FormatException(
          'A backup file failed its integrity check.',
        );
      }
      totalBytes += content.length;
      if (totalBytes > maxExpandedBytes) {
        throw const FormatException('The backup expands beyond 48 MiB.');
      }
      payloads[path] = content;
      if (mode is int) projectModes[path] = mode;
    }
    _validatePayloads(payloads);
    final room = RoomDef.fromJson(
      (decoded['room'] as Map).cast<String, Object?>(),
    );
    final bots = (decoded['bots'] as List)
        .map((value) {
          if (value is! Map) {
            throw const FormatException('The backup bot list is invalid.');
          }
          return BotDef.fromJson(value.cast<String, Object?>());
        })
        .toList(growable: false);
    final ids = bots.map((bot) => bot.id).toSet();
    if (ids.length != bots.length ||
        room.bots.any((slot) => !ids.contains(slot.botId))) {
      throw const FormatException('The backup room roster is inconsistent.');
    }
    final stateBytes = payloads['room/state.json'];
    final stateValue = stateBytes == null
        ? const <String, Object?>{}
        : jsonDecode(utf8.decode(stateBytes));
    if (stateValue is! Map) {
      throw const FormatException('The saved room state is invalid.');
    }
    if (stateBytes != null) {
      final version = stateValue['version'];
      final validState = version == 0
          ? stateValue['room'] is Map && stateValue['events'] is List
          : version == 1 &&
                stateValue['engine'] is Map &&
                (stateValue['engine'] as Map)['room'] is Map &&
                (stateValue['engine'] as Map)['log'] is List;
      if (!validState) {
        throw const FormatException('The saved room state format is invalid.');
      }
      if (!_checkpointIsPaused(stateValue.cast<String, Object?>())) {
        throw const FormatException(
          'The backup contains a running room checkpoint.',
        );
      }
    }
    return PortableRoomBackup._(
      room,
      bots,
      stateValue.cast<String, Object?>(),
      Map.unmodifiable(payloads),
      Map.unmodifiable(projectModes),
    );
  }

  /// Write only into a new directory; existing user folders are never merged.
  Future<Directory> restoreFiles(Directory target) async {
    if (await FileSystemEntity.type(target.path, followLinks: false) !=
        FileSystemEntityType.notFound) {
      throw FileSystemException(
        'The restore folder already exists.',
        target.path,
      );
    }
    await target.create(recursive: true);
    try {
      await _setPermissions(target.path, _privateExecutableMode);
      for (final entry in files.entries.where(
        (e) => e.key.startsWith('project/'),
      )) {
        final relative = entry.key.substring('project/'.length);
        final output = File('${target.path}/$relative');
        await output.parent.create(recursive: true);
        await _setPermissions(output.parent.path, _privateExecutableMode);
        await output.writeAsBytes(entry.value, flush: true);
        final executable = (_projectModes[entry.key] ?? 0) != 0;
        await _setPermissions(
          output.path,
          executable ? _privateExecutableMode : _privateFileMode,
        );
      }
      return target;
    } catch (_) {
      await target.delete(recursive: true);
      rethrow;
    }
  }

  static Future<void> _setPermissions(String path, int mode) async {
    if (!Platform.isLinux) return;
    final result = await Process.run('chmod', [mode.toRadixString(8), path]);
    if (result.exitCode != 0) {
      throw const FileSystemException(
        'Could not protect restored project files.',
      );
    }
  }

  Future<void> restoreHistory(String roomId) async {
    final directory = await RoomArchive.directory(roomId);
    if (await FileSystemEntity.type(directory.path, followLinks: false) !=
        FileSystemEntityType.notFound) {
      throw const FileSystemException(
        'The restored room history already exists.',
      );
    }
    await directory.create(recursive: true);
    try {
      for (final entry in files.entries.where(
        (e) => e.key.startsWith('room/'),
      )) {
        final relative = entry.key.substring('room/'.length);
        final output = File('${directory.path}/$relative');
        await output.parent.create(recursive: true);
        await output.writeAsBytes(entry.value, flush: true);
      }
      if (Platform.isLinux) {
        final result = await Process.run('chmod', ['700', directory.path]);
        if (result.exitCode != 0) {
          throw const FileSystemException(
            'Could not protect the restored history.',
          );
        }
        for (final entry in files.entries.where(
          (e) => e.key.startsWith('room/'),
        )) {
          final relative = entry.key.substring('room/'.length);
          final fileResult = await Process.run('chmod', [
            '600',
            '${directory.path}/$relative',
          ]);
          if (fileResult.exitCode != 0) {
            throw const FileSystemException(
              'Could not protect restored history files.',
            );
          }
        }
      }
    } catch (_) {
      await directory.delete(recursive: true);
      rethrow;
    }
  }

  Future<File?> restoreProjectMemory(String projectPath) async {
    final bytes = files['memory/project.json'];
    if (bytes == null) return null;
    if (bytes.length > maxMemoryFileBytes) {
      throw const FormatException('Project memory exceeds 16 MiB.');
    }
    final value = jsonDecode(utf8.decode(bytes));
    if (value is! Map || value['version'] is! int || value['entries'] is! Map) {
      throw const FormatException('The project memory backup is invalid.');
    }
    final canonical = await Directory(projectPath).resolveSymbolicLinks();
    final scopeHash = sha256
        .convert(utf8.encode('project:$canonical'))
        .toString();
    final updated = value.cast<String, Object?>()..['scope_hash'] = scopeHash;
    final support = await getApplicationSupportDirectory();
    final root = Directory('${support.path}/project-memory');
    if (await FileSystemEntity.type(root.path, followLinks: false) ==
        FileSystemEntityType.link) {
      throw const FormatException('Project memory cannot be a symbolic link.');
    }
    await root.create(recursive: true);
    if (Platform.isLinux) {
      final result = await Process.run('chmod', ['700', root.path]);
      if (result.exitCode != 0) {
        throw const FileSystemException(
          'Could not protect restored project memory.',
        );
      }
    }
    final output = File('${root.path}/$scopeHash.json');
    if (await FileSystemEntity.type(output.path, followLinks: false) !=
        FileSystemEntityType.notFound) {
      throw const FileSystemException(
        'Project memory for this path already exists.',
      );
    }
    await output.create(exclusive: true);
    try {
      await output.writeAsString(jsonEncode(updated), flush: true);
      if (Platform.isLinux) {
        final result = await Process.run('chmod', ['600', output.path]);
        if (result.exitCode != 0) {
          throw const FileSystemException(
            'Could not protect restored project memory.',
          );
        }
      }
    } catch (_) {
      await output.delete();
      rethrow;
    }
    return output;
  }

  ({RoomDef room, List<BotDef> bots}) restoreDefinitions({
    required String newRoomId,
    required String newProjectPath,
  }) {
    final idMap = {for (final bot in bots) bot.id: genId('bot')};
    final restoredBots = bots.map((bot) {
      final json = _withoutCredentials(bot.toJson())..['id'] = idMap[bot.id];
      return BotDef.fromJson(json);
    }).toList();
    final roomJson = _withoutCredentials(room.toJson())
      ..['id'] = newRoomId
      ..['path'] = newProjectPath
      ..['workspace'] = {
        'kind': 'phone',
        'path': newProjectPath,
        'writable': true,
        'commands': false,
      }
      ..['bots'] = [
        for (final slot in room.bots)
          {'bot_id': idMap[slot.botId] ?? genId('bot')},
      ];
    return (room: RoomDef.fromJson(roomJson), bots: restoredBots);
  }

  static Future<Map<String, List<int>>> readProjectFiles(
    Directory root, {
    bool Function()? cancelled,
    void Function(int count)? onProgress,
    Future<List<int>> Function(String root, String file)? readFile,
    void Function(String path, int mode)? onFileMode,
    int maxBytes = maxExpandedBytes,
    int maxFileCount = maxFiles,
  }) async {
    final canonicalRoot = await root.resolveSymbolicLinks();
    final files = <String, List<int>>{};
    var total = 0;
    var count = 0;
    var visited = 0;
    await for (final entity in root.list(recursive: true, followLinks: false)) {
      if (++visited > maxFiles * 8) {
        throw const FormatException(
          'The selected project has too many entries.',
        );
      }
      if (cancelled?.call() == true) throw StateError('Backup canceled.');
      final kind = await FileSystemEntity.type(entity.path, followLinks: false);
      if (kind == FileSystemEntityType.link) {
        throw const FormatException(
          'Project folders containing symbolic links cannot be backed up.',
        );
      }
      if (kind != FileSystemEntityType.file) continue;
      final file = File(entity.path);
      final canonicalFile = await file.resolveSymbolicLinks();
      if (!canonicalFile.startsWith(
        '$canonicalRoot${Platform.pathSeparator}',
      )) {
        throw const FormatException(
          'A project file leaves the selected folder.',
        );
      }
      final relative = canonicalFile.substring(canonicalRoot.length + 1);
      if (_isSecretPath(relative)) continue;
      final before = await file.stat();
      if (before.type != FileSystemEntityType.file) {
        throw const FormatException('A project file is not a regular file.');
      }
      final content = await (readFile ?? DesktopFiles.readProjectFile)(
        canonicalRoot,
        entity.path,
      );
      final after = await file.stat();
      if (before.size != after.size ||
          before.modified != after.modified ||
          before.mode != after.mode ||
          content.length != after.size) {
        throw const FileSystemException(
          'A project file changed while being read.',
        );
      }
      final length = content.length;
      if (length > maxProjectFileBytes) {
        throw FormatException('Project file exceeds 16 MiB: $relative');
      }
      total += length;
      if (total > maxBytes || files.length >= maxFileCount) {
        throw const FormatException(
          'The selected project exceeds backup limits.',
        );
      }
      files['project/$relative'] = content;
      onFileMode?.call('project/$relative', before.mode);
      onProgress?.call(++count);
    }
    _validatePayloads(files);
    return files;
  }

  static Future<String?> chooseExportPath() =>
      _picker.invokeMethod<String>('choose_backup_export');

  static Future<String?> chooseImportPath() =>
      _picker.invokeMethod<String>('choose_backup_import');

  static Future<void> writePrivateFile(String path, List<int> bytes) async {
    if (!Platform.isLinux || bytes.length > maxBundleBytes) {
      throw const FormatException('This backup file cannot be saved here.');
    }
    final saved = await _picker.invokeMethod<bool>('write_backup', {
      'path': path,
      'bytes': bytes,
    });
    if (saved != true) {
      throw const FileSystemException('Could not save backup.');
    }
  }

  static Future<List<int>> readBackupFile(String path) async {
    if (!Platform.isLinux) {
      throw const FormatException('Choose a regular local backup file.');
    }
    final bytes = await _picker.invokeMethod<List<int>>('read_backup', {
      'path': path,
    });
    if (bytes == null || bytes.length > maxBundleBytes) {
      throw const FormatException('The portable backup exceeds 64 MiB.');
    }
    return bytes;
  }

  static bool _isSecretPath(String path) {
    final parts = path.split(Platform.pathSeparator);
    final name = parts.last.toLowerCase();
    return parts.contains('.git') ||
        parts.contains('.ssh') ||
        parts.contains('.aws') ||
        parts.contains('.kube') ||
        name == '.env' ||
        name.startsWith('.env.') ||
        name.contains('credential') ||
        name.contains('token') ||
        name.contains('secret') ||
        name.contains('private_key') ||
        name == '.envrc' ||
        const {'.netrc', '.npmrc', '.pypirc'}.contains(name) ||
        (parts.contains('.docker') && name == 'config.json') ||
        (parts.contains('.config') && parts.contains('gcloud')) ||
        name.endsWith('.pem') ||
        name.endsWith('.key') ||
        name.endsWith('.p12') ||
        name.endsWith('.pfx') ||
        name.endsWith('.ppk') ||
        name.endsWith('.jks') ||
        name.endsWith('.asc') ||
        const {'id_rsa', 'id_ed25519', 'id_ecdsa', 'id_dsa'}.contains(name);
  }

  static Future<List<int>> _readStable(File file) async {
    if (await FileSystemEntity.type(file.path, followLinks: false) !=
        FileSystemEntityType.file) {
      throw const FormatException('A backup source is not a regular file.');
    }
    final before = await file.stat();
    if (before.type != FileSystemEntityType.file ||
        before.size > maxFileBytes) {
      throw const FormatException(
        'A backup source is not a bounded regular file.',
      );
    }
    final bytes = Platform.isLinux
        ? await _picker.invokeMethod<List<int>>('read_backup', {
            'path': file.path,
          })
        : await file.readAsBytes();
    if (bytes == null) {
      throw const FileSystemException('Could not read a backup source file.');
    }
    final after = await file.stat();
    if (before.size != after.size ||
        before.modified != after.modified ||
        bytes.length != after.size) {
      throw const FileSystemException(
        'A file changed while the backup was being read.',
      );
    }
    return bytes;
  }

  static Map<String, Object?> _withoutCredentials(Map<String, Object?> json) {
    final safe = Map<String, Object?>.from(json);
    if (safe.containsKey('apiKey')) safe['apiKey'] = '';
    if (safe['workspace'] is Map) {
      final workspace = (safe['workspace'] as Map).cast<String, Object?>();
      workspace['path'] = '';
      for (final key in [
        'host',
        'username',
        'password',
        'private_key',
        'passphrase',
        'fingerprint',
      ]) {
        workspace[key] = '';
      }
      if (workspace['kind'] == 'ssh') {
        workspace['kind'] = 'phone';
        workspace['host'] = '';
        workspace['username'] = '';
        workspace['port'] = 22;
      }
      safe['workspace'] = workspace;
    }
    return safe;
  }

  static void _validatePayloads(Map<String, List<int>> payloads) {
    if (payloads.length > maxFiles) {
      throw const FormatException('The backup has an invalid file count.');
    }
    var total = 0;
    for (final entry in payloads.entries) {
      _validatePath(entry.key);
      if (entry.value.length > maxFileBytes) {
        throw const FormatException('A backup file exceeds 32 MiB.');
      }
      total += entry.value.length;
    }
    if (total > maxExpandedBytes) {
      throw const FormatException('The backup expands beyond 48 MiB.');
    }
  }

  static void _validatePath(String path) {
    final parts = path.split('/');
    if (path.isEmpty ||
        path.length > 4096 ||
        path.startsWith('/') ||
        path.contains('\\') ||
        parts.length > 128 ||
        parts.any(
          (part) =>
              part.isEmpty || part.length > 255 || part == '.' || part == '..',
        ) ||
        !(path.startsWith('project/') ||
            path.startsWith('room/') ||
            path.startsWith('memory/'))) {
      throw const FormatException('The backup contains an unsafe file path.');
    }
  }

  static bool _checkpointIsPaused(Map<String, Object?> state) {
    final rawRoom = state['version'] == 0
        ? state['room']
        : (state['engine'] as Map?)?['room'];
    if (rawRoom is! Map) return false;
    return const {
      'assembling',
      'paused',
      'completed',
      'aborted',
    }.contains(rawRoom['phase']);
  }
}
