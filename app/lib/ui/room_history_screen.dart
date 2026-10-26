import 'dart:io';
import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import '../orgbot/backup.dart';
import '../orgbot/library.dart';
import '../orgbot/models.dart';
import '../orgbot/room_check.dart' show expandHome;
import '../state/room_archive.dart';

class RoomHistoryScreen extends StatefulWidget {
  final String roomId, roomName;
  final Library library;
  const RoomHistoryScreen({
    super.key,
    required this.roomId,
    required this.roomName,
    required this.library,
  });
  @override
  State<RoomHistoryScreen> createState() => _RoomHistoryScreenState();
}

class _RoomHistoryScreenState extends State<RoomHistoryScreen> {
  List<File> _files = [];
  File? _selected;
  String _transcript = 'Loading saved runs…';
  bool _backupBusy = false;
  bool _cancelBackup = false;
  String? _backupStatus;

  @override
  void dispose() {
    _cancelBackup = true;
    super.dispose();
  }

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final files = await RoomArchive.list(widget.roomId);
      if (!mounted) return;
      setState(() => _files = files);
      if (files.isEmpty) {
        setState(() => _transcript = 'No saved runs yet.');
      } else {
        await _read(files.first);
      }
    } catch (e) {
      if (mounted) setState(() => _transcript = 'Could not open history: $e');
    }
  }

  String _label(File file) {
    final micros = int.tryParse(file.path.split('/').last.split('.').first);
    return micros == null
        ? file.path.split('/').last
        : DateTime.fromMicrosecondsSinceEpoch(
            micros,
          ).toLocal().toString().split('.').first;
  }

  Future<void> _exportBackup() async {
    if (_backupBusy) return;
    if (!_sourceStopped) {
      _message('Stop the room before creating a backup.');
      return;
    }
    setState(() {
      _backupBusy = true;
      _cancelBackup = false;
      _backupStatus = 'Collecting saved room data…';
    });
    try {
      final room = widget.library.rooms.firstWhere(
        (r) => r.id == widget.roomId,
      );
      final history = await PortableRoomBackup.roomHistoryFiles(room.id);
      _checkBackupCancelled();
      final stateBytes = history['room/state.json'];
      final stateValue = stateBytes == null
          ? const <String, Object?>{}
          : jsonDecode(utf8.decode(stateBytes));
      if (stateValue is! Map) {
        throw const FormatException('Saved room state is invalid.');
      }
      final files = <String, List<int>>{...history};
      final projectFileModes = <String, int>{};
      var payloadBytes = files.values.fold<int>(
        0,
        (sum, bytes) => sum + bytes.length,
      );
      if (room.path.trim().isNotEmpty) {
        final project = Directory(expandHome(room.path));
        if (!await project.exists()) {
          throw const FileSystemException(
            'The configured project folder is unavailable.',
          );
        }
        files.addAll(
          await PortableRoomBackup.readProjectFiles(
            project,
            cancelled: () => _cancelBackup,
            maxBytes: PortableRoomBackup.maxExpandedBytes - payloadBytes,
            maxFileCount: PortableRoomBackup.maxFiles - files.length,
            onFileMode: (path, mode) => projectFileModes[path] = mode,
            onProgress: (count) {
              if (mounted) {
                setState(
                  () => _backupStatus = 'Reading project files · $count',
                );
              }
            },
          ),
        );
        payloadBytes = files.values.fold<int>(
          0,
          (sum, bytes) => sum + bytes.length,
        );
        final memory = await PortableRoomBackup.projectMemoryFiles(room.path);
        final memoryBytes = memory.values.fold<int>(
          0,
          (sum, bytes) => sum + bytes.length,
        );
        if (payloadBytes + memoryBytes > PortableRoomBackup.maxExpandedBytes) {
          throw const FormatException(
            'The selected project and history exceed backup limits.',
          );
        }
        files.addAll(memory);
      }
      _checkBackupCancelled();
      final bots = room.bots
          .map((slot) => widget.library.botById(slot.botId))
          .whereType<BotDef>()
          .toList();
      final bundle = await PortableRoomBackup.export(
        room: room,
        bots: bots,
        state: stateValue.cast<String, Object?>(),
        files: files,
        projectFileModes: projectFileModes,
        isStopped: _sourceStopped,
      );
      _checkBackupCancelled();
      if (!mounted) return;
      setState(
        () => _backupStatus = 'Choose where to save the private backup.',
      );
      final path = await PortableRoomBackup.chooseExportPath();
      if (path == null) return;
      _checkBackupCancelled();
      setState(() => _backupStatus = 'Saving backup…');
      await PortableRoomBackup.writePrivateFile(path, bundle);
      if (mounted) _message('Private room backup saved.');
    } catch (error) {
      if (mounted) {
        _message(
          _cancelBackup
              ? 'Backup canceled.'
              : 'Could not create backup: $error',
        );
      }
    } finally {
      if (mounted) {
        setState(() {
          _backupBusy = false;
          _backupStatus = null;
        });
      }
    }
  }

  Future<void> _importBackup() async {
    if (_backupBusy) return;
    setState(() {
      _backupBusy = true;
      _cancelBackup = false;
      _backupStatus = 'Choose a backup to restore.';
    });
    String? createdProject;
    String? createdHistory;
    File? createdMemory;
    String? addedRoomId;
    final addedBotIds = <String>[];
    try {
      final path = await PortableRoomBackup.chooseImportPath();
      if (path == null) return;
      _checkBackupCancelled();
      setState(() => _backupStatus = 'Checking backup integrity…');
      final bytes = await PortableRoomBackup.readBackupFile(path);
      _checkBackupCancelled();
      final backup = PortableRoomBackup.parse(bytes);
      _checkBackupCancelled();
      final roomId = genId('room');
      final support = await getApplicationSupportDirectory();
      final root = Directory('${support.path}/restored');
      await root.create(recursive: true);
      if (Platform.isLinux) {
        final chmod = await Process.run('chmod', ['700', root.path]);
        if (chmod.exitCode != 0) {
          throw const FileSystemException(
            'Could not protect restored projects.',
          );
        }
      }
      final project = Directory('${root.path}/$roomId');
      setState(() => _backupStatus = 'Restoring into a new project folder…');
      await backup.restoreFiles(project);
      createdProject = project.path;
      _checkBackupCancelled();
      await backup.restoreHistory(roomId);
      createdHistory = (await RoomArchive.directory(roomId)).path;
      _checkBackupCancelled();
      createdMemory = await backup.restoreProjectMemory(project.path);
      _checkBackupCancelled();
      final restored = backup.restoreDefinitions(
        newRoomId: roomId,
        newProjectPath: project.path,
      );
      widget.library.bots.addAll(restored.bots);
      addedBotIds.addAll(restored.bots.map((bot) => bot.id));
      widget.library.rooms.add(restored.room);
      addedRoomId = roomId;
      setState(() => _backupStatus = 'Saving restored room…');
      widget.library.error = null;
      await widget.library.save();
      if (widget.library.error != null) {
        throw StateError(widget.library.error!);
      }
      if (mounted) {
        _message('Room restored paused. Open it when you are ready.');
      }
    } catch (error) {
      if (addedRoomId != null) {
        widget.library.rooms.removeWhere((room) => room.id == addedRoomId);
        widget.library.bots.removeWhere((bot) => addedBotIds.contains(bot.id));
        await widget.library.save();
      }
      if (createdMemory != null && await createdMemory.exists()) {
        await createdMemory.delete();
      }
      if (createdHistory != null) {
        final history = Directory(createdHistory);
        if (await history.exists()) await history.delete(recursive: true);
      }
      if (createdProject != null) {
        final project = Directory(createdProject);
        if (await project.exists()) await project.delete(recursive: true);
      }
      if (mounted) {
        _message(
          _cancelBackup
              ? 'Restore canceled.'
              : 'Could not restore backup: $error',
        );
      }
    } finally {
      if (mounted) {
        setState(() {
          _backupBusy = false;
          _backupStatus = null;
        });
      }
    }
  }

  void _checkBackupCancelled() {
    if (_cancelBackup) throw StateError('Backup canceled.');
  }

  bool get _sourceStopped {
    final session = widget.library.sessions[widget.roomId];
    return session == null ||
        !session.isRunning ||
        (session.store != null && !session.store!.isActive);
  }

  void _message(String text) {
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(text)));
  }

  Future<void> _read(File file) async {
    setState(() {
      _selected = file;
      _transcript = 'Reading…';
    });
    final messages = <String, String>{};
    final names = <String, String>{};
    String goal = '';
    String phase = 'interrupted or closed';
    try {
      await for (final message in RoomArchive.read(file)) {
        if (message['type'] == 'snapshot') {
          messages.clear();
          for (final agent in message['roster'] as List? ?? []) {
            names[agent['id'] as String] = agent['handle'] as String;
          }
          goal =
              ((message['room'] as Map?)?['spec'] as Map?)?['goal']
                  as String? ??
              '';
        }
        if (message['type'] == 'phase') phase = message['phase'] as String;
        final event = (message['envelope'] as Map?)?['event'] as Map?;
        if (event?['event'] == 'message_posted') {
          final chat = event!['message'] as Map;
          final author = chat['author'] as Map;
          final name =
              author['name'] ??
              names[author['agent']] ??
              names[author['id']] ??
              author.toString();
          messages[chat['id'] as String] = '$name\n${chat['body']}';
        }
      }
      if (mounted && _selected == file) {
        setState(
          () => _transcript =
              'Goal: $goal\nLast recorded phase: $phase\n\n${messages.values.join('\n\n')}',
        );
      }
    } catch (e) {
      if (mounted) setState(() => _transcript = 'Could not read this run: $e');
    }
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(
      title: Text('${widget.roomName} · history'),
      actions: [
        if (Platform.isLinux)
          IconButton(
            tooltip: _backupBusy
                ? 'Cancel backup operation'
                : 'Export portable backup',
            onPressed:
                _backupBusy && (_backupStatus?.startsWith('Saving') ?? false)
                ? null
                : _backupBusy
                ? () => setState(() => _cancelBackup = true)
                : _exportBackup,
            icon: _backupBusy
                ? const Icon(Icons.close_rounded)
                : const Icon(Icons.save_alt_rounded),
          ),
        if (Platform.isLinux)
          IconButton(
            tooltip: 'Import portable backup',
            onPressed: _backupBusy ? null : _importBackup,
            icon: const Icon(Icons.restore_rounded),
          ),
      ],
    ),
    body: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (_backupBusy)
          LinearProgressIndicator(
            semanticsLabel: _backupStatus ?? 'Backup operation in progress',
          ),
        if (_backupStatus != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
            child: Text(_backupStatus!),
          ),
        const Padding(
          padding: EdgeInsets.all(16),
          child: Text(
            'Earlier saved sessions. Open the room to continue its conversation.',
          ),
        ),
        if (Platform.isLinux)
          const Padding(
            padding: EdgeInsets.fromLTRB(16, 0, 16, 12),
            child: Text(
              'Backups include private history, project memory, and selected project files. Pasted secrets may be present. Provider keys stay in this device’s keyring and are not included; sign in again on another device.',
            ),
          ),
        if (_files.isNotEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: DropdownButton<File>(
              isExpanded: true,
              value: _selected,
              items: [
                for (final file in _files)
                  DropdownMenuItem(value: file, child: Text(_label(file))),
              ],
              onChanged: (file) {
                if (file != null) _read(file);
              },
            ),
          ),
        Expanded(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(16),
            child: SelectableText(_transcript),
          ),
        ),
      ],
    ),
  );
}
