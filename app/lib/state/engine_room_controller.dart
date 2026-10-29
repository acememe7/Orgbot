// Runs a room on the real engine and exposes it to the UI.
//
// Owns the lifecycle the screen should not have to think about: generate the
// room file, start `orgbot serve`, connect the WebSocket, and tear all of it
// down again when the screen closes.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:async';
import 'dart:convert';
import '../workspace/workspace.dart';
import '../workspace/attachments.dart';
import '../orgbot/desktop_integrations.dart';
import '../orgbot/allowances.dart';
import '../protocol/models.dart' show ChatFile;

import 'package:flutter/foundation.dart';

import 'dart:io';
import 'dart:isolate';

import '../engine/core_server.dart';
import '../engine/embedded_engine.dart';
import '../net/room_client.dart';
import '../orgbot/library.dart';
import '../orgbot/models.dart';
import '../orgbot/model_catalog.dart';
import '../orgbot/room_check.dart';
import '../orgbot/room_file.dart';
import '../orgbot/skills.dart';
import 'room_store.dart';
import 'room_archive.dart';
import 'background_rooms.dart';

/// How far along starting a room is.
enum RoomLaunch {
  idle,
  generating,
  starting,
  connecting,
  running,
  failed,
  closed,
}

/// Starts a room on the engine and keeps the UI informed while it happens.
class EngineRoomController extends ChangeNotifier {
  EngineRoomController({required this.room, required this.library});

  final RoomDef room;
  final Library library;

  RoomLaunch phase = RoomLaunch.idle;

  /// Why the room could not start, when it could not.
  String? error;

  /// Bots left out of the room, with the reason — a missing key, say.
  List<String> skipped = const [];

  /// Reasons this roster may never finish its work. Reported rather than
  /// refused: it is your room, and a room that only chats is a fair use of it.
  List<RoomIssue> issues = const [];

  CoreServer? _server;

  /// Brings a dropped room connection back. A room's service can restart, or
  /// the computer can sleep; without this the screen stayed disconnected,
  /// showed no updates and swallowed messages while the room kept working.
  StreamSubscription<LinkState>? _linkWatch;
  Timer? _reconnectTimer;
  int _reconnectAttempts = 0;
  RoomLink? _client;
  RoomStore? _store;

  /// Set when the room runs inside this process rather than as a subprocess.
  EmbeddedEngine? _embedded;
  int _embeddedRoom = 0;
  Timer? _pump;
  Timer? _renewalTimer;
  Future<void>? _refreshingCredentials;
  Map<String, String> _providerEnvironment = {};
  WorkspaceConnection? workspace;
  bool _disposed = false;
  int _generation = 0;
  int _memoryRequestSequence = 0;
  final Map<String, Completer<Map<String, Object?>>> _memoryRequests = {};
  Set<String> _memoryProjects = const {};
  TrustHost? trustHost;
  bool _controlBusy = false;
  final _workspaceCalls = <int, WorkspaceConnection>{};
  RoomArchive? _archive;
  StreamSubscription<Map<String, Object?>>? _archiveSubscription;
  bool? _backgroundActive;
  Future<void> _backgroundWriting = Future.value();
  String draft = '';

  Future<void> _background(bool active) {
    if (_backgroundActive == active) return _backgroundWriting;
    _backgroundActive = active;
    return _backgroundWriting = _backgroundWriting
        .catchError((Object _) {})
        .then((_) => BackgroundRooms.update(room.id, active));
  }

  void _changed() {
    if (_disposed) return;
    final active = isRunning && _store?.isActive == true;
    unawaited(
      _background(active).catchError((Object _) {
        error =
            'Background execution is unavailable. Open the app and resume the room.';
        _client?.pause(reason: 'Background execution unavailable');
        if (!_disposed) notifyListeners();
      }),
    );
    notifyListeners();
  }

  /// Whether a room here runs in-process.
  ///
  /// A phone cannot spawn a subprocess, so it loads the engine as a library
  /// instead. Same engine either way — only how it is started differs.
  static bool get runsInProcess => Platform.isAndroid || Platform.isIOS;

  /// The live room state, once connected.
  RoomStore? get store => _store;

  /// Projects this desktop owner has explicitly configured for memory sharing.
  Set<String> get memoryProjects => Set.unmodifiable(_memoryProjects);

  /// Whether the room is up and usable.
  bool get isRunning => phase == RoomLaunch.running;

  /// A short line describing what is happening, for the UI.
  String get statusLine {
    switch (phase) {
      case RoomLaunch.idle:
        return 'Ready to start';
      case RoomLaunch.generating:
        return 'Preparing the room…';
      case RoomLaunch.starting:
        return 'Starting the engine…';
      case RoomLaunch.connecting:
        return 'Connecting…';
      case RoomLaunch.running:
        return _store?.runStateLabel ?? 'Running';
      case RoomLaunch.failed:
        return error ?? 'Failed to start';
      case RoomLaunch.closed:
        return 'Saved';
    }
  }

  void _to(RoomLaunch next) {
    phase = next;
    if (!_disposed) notifyListeners();
  }

  /// Generate the room file, start the engine, and connect to it.
  Future<void> start() async {
    if (_disposed) return;
    if (![
      RoomLaunch.idle,
      RoomLaunch.failed,
      RoomLaunch.closed,
    ].contains(phase)) {
      return;
    }
    final generation = ++_generation;
    error = null;
    _to(RoomLaunch.generating);

    try {
      final savedState = await RoomArchive.stateFile(room.id);
      final restoring = await savedState.exists();
      if (_store == null && restoring) {
        final preview = LocalRoomClient();
        _client = preview;
        _store = RoomStore(preview, notificationRoomId: room.id)
          ..restore(await RoomArchive.savedFrames(room.id));
        _store!.addListener(_changed);
        if (!_disposed) notifyListeners();
      }
      await _background(true);
      final bots = room.bots
          .map((slot) => library.botById(slot.botId))
          .whereType<BotDef>()
          .toList();
      if (bots.isEmpty) {
        throw EngineStartError('This room has no bots in it.');
      }

      // Surfaced now, because a room with no inspector looks fine and then
      // parks every task in review.
      issues = checkRoom(room, bots);

      // Subscription tokens last hours, so renew before the room starts
      // rather than letting a bot fail its first turn on a stale one.
      if (runsInProcess) await _renewSignIns(bots);
      if (_disposed || generation != _generation) return;
      if (runsInProcess) {
        // Older editors displayed the first catalog entry but saved an empty id.
        // Repair only that missing selection; explicit model choices stay intact.
        for (final bot in bots.where(
          (b) => b.provider.signsInOnDevice && b.model.trim().isEmpty,
        )) {
          final models = await ModelCatalog.load(bot.provider);
          if (models.source != ModelSource.live || models.options.isEmpty) {
            throw EngineStartError(
              'Choose a model for @${bot.handle.isEmpty ? bot.name : bot.handle} in Edit bot. ${models.problem ?? 'The provider model list is unavailable.'}',
            );
          }
          bot.model = models.options.first.id;
        }
        final connection = WorkspaceConnection(room.workspace.copy(), room.id);
        workspace = connection;
        if (!restoring) await connection.connect(trust: trustHost);
        if (_disposed || generation != _generation) {
          await connection.close();
          return;
        }
        if (!restoring) {
          room.workspace.fingerprint = connection.config.fingerprint;
        }
        await library.save();
      }

      final projectId = await _projectScope(room.path);
      final knownProjects = <String>{
        for (final knownRoom in library.rooms)
          await _projectScope(knownRoom.path) ?? 'room:${knownRoom.id}',
      };
      if (projectId != null) knownProjects.add(projectId);
      _memoryProjects = knownProjects;
      final file = buildRoomFile(
        room,
        bots,
        credentials: ModelCatalog.credentials,
        workspace: workspace == null
            ? !runsInProcess &&
                      room.path.trim().isNotEmpty &&
                      bots.any((bot) => bot.provider.needsApiKey)
                  ? {
                      'label': 'Project files',
                      'root': Directory(expandHome(room.path)).absolute.path,
                      'writable': true,
                      'commands': room.workspace.commands,
                    }
                  : null
            : {
                ...workspace!.description,
                if (restoring)
                  'root': room.workspace.path.isNotEmpty
                      ? room.workspace.path
                      : '${await WorkspaceConnection.phoneBase()}/${room.id}',
              },
        userName: library.userName,
        userProfile: library.userProfile,
        statePath: savedState.path,
        projectId: projectId,
        skillsPath: await unpackBundledSkills(),
        priorBoards: await RoomArchive.otherBoards(room.id),
        desktopIntegrations: !runsInProcess
            ? await DesktopIntegrations.discover()
            : null,
      );
      if (!runsInProcess) {
        file.environment.addAll(
          await DesktopAllowances.environment(projectIds: knownProjects),
        );
      }
      skipped = file.skipped;
      if (file.toml.contains('[[bots]]') == false) {
        throw EngineStartError(
          skipped.isEmpty
              ? 'No usable bots in this room.'
              : 'No usable bots: ${skipped.join('; ')}',
        );
      }

      _archive = await RoomArchive.open(room.id, (message) {
        error = message;
        if (!_disposed) notifyListeners();
      });
      if (_disposed || generation != _generation) {
        await _archive?.close();
        return;
      }
      _to(RoomLaunch.starting);
      if (runsInProcess) {
        await _startEmbedded(file);
      } else {
        await _startSubprocess(file);
      }
      if (_disposed || generation != _generation) {
        await _teardown();
        return;
      }
      _to(RoomLaunch.running);
      _changed();
    } catch (e) {
      error = e.toString();
      await _teardown();
      _to(RoomLaunch.failed);
    }
  }

  /// Refresh any sign-in this room needs that is close to expiry.
  ///
  /// Keep transient failures explicit, without deleting the saved sign-in.
  Future<void> _renewSignIns(List<BotDef> bots) async {
    final credentials = ModelCatalog.credentials;
    if (credentials == null) return;
    await credentials.load();
    for (final provider in bots.map((b) => b.provider).toSet()) {
      if (!credentials.isSignedIn(provider)) continue;
      try {
        await credentials.freshToken(provider);
      } catch (_) {
        throw EngineStartError(
          'Could not refresh ${provider.label}. Check the connection and retry.',
        );
      }
    }
  }

  /// Run the room as a child process and talk to it over a socket.
  Future<void> _startSubprocess(RoomFile file) async {
    final server = await startRoom(file, roomId: room.id);
    if (_disposed) {
      await server.detach();
      return;
    }
    _server = server;

    _to(RoomLaunch.connecting);
    final client = RoomClient();
    await _client?.close();
    _store?.removeListener(_changed);
    _store?.dispose();
    _client = client;
    _archiveSubscription = client.messages.listen((message) {
      if (message['type'] == 'memory_result') {
        final id = message['request_id'] as String?;
        final pending = id == null ? null : _memoryRequests.remove(id);
        pending?.complete(Map<String, Object?>.from(message));
        return;
      }
      _archive?.append([message]);
    });
    final store = RoomStore(client, notificationRoomId: room.id);
    _store = store;
    store.addListener(_changed);

    await client.connect(server.url, token: server.token);
    if (client.current == LinkState.error) {
      throw EngineStartError('Connected to the engine but the socket failed.');
    }
    _reconnectAttempts = 0;
    _linkWatch = client.connectionState.listen((state) {
      if (state == LinkState.connected) {
        _reconnectAttempts = 0;
      } else if (state == LinkState.closed || state == LinkState.error) {
        _scheduleReconnect(file, client);
      }
    });
  }

  void _scheduleReconnect(RoomFile file, RoomClient client) {
    if (_disposed || _reconnectTimer != null || !identical(_client, client)) {
      return;
    }
    final seconds = 1 << _reconnectAttempts.clamp(0, 4);
    _reconnectAttempts++;
    _reconnectTimer = Timer(
      Duration(seconds: seconds.clamp(1, 15)),
      () => unawaited(_reconnect(file, client)),
    );
  }

  Future<void> _reconnect(RoomFile file, RoomClient client) async {
    _reconnectTimer = null;
    if (_disposed || !identical(_client, client) || !isRunning) return;
    try {
      // Reuses the room's service when it is up, or starts it again.
      final server = await startRoom(file, roomId: room.id);
      if (_disposed || !identical(_client, client)) return;
      _server = server;
      await client.connect(server.url, token: server.token);
    } catch (_) {
      // Tried again below.
    }
    if (client.current != LinkState.connected) _scheduleReconnect(file, client);
  }

  /// Run the room inside this process, through the engine library.
  Future<void> _startEmbedded(RoomFile file) async {
    final engine = EmbeddedEngine.load();
    _embedded = engine;
    _providerEnvironment = Map.of(file.environment);
    _embeddedRoom = await _openEmbeddedRoom(file.toml, file.environment);
    if (_disposed) {
      engine.closeRoom(_embeddedRoom);
      _embeddedRoom = 0;
      return;
    }

    _to(RoomLaunch.connecting);
    // The store is fed the same wire messages the socket would carry, so
    // nothing above it knows which transport it got.
    final client = LocalRoomClient(
      onSend: (message) {
        if (_embeddedRoom == 0) return;
        if (!engine.control(_embeddedRoom, message)) {
          error = 'Engine control was not accepted. Try again.';
          if (!_disposed) notifyListeners();
        } else if (message['type'] == 'pause') {
          unawaited(workspace?.close());
        }
      },
    );
    await _client?.close();
    _store?.removeListener(_changed);
    _store?.dispose();
    _client = client;
    _archiveSubscription = client.messages.listen(
      (message) => _archive?.append([message]),
    );
    final store = RoomStore(client, notificationRoomId: room.id);
    _store = store;
    store.addListener(_changed);

    final snapshot = engine.snapshot(_embeddedRoom);
    if (snapshot != null) client.deliver(snapshot);
    // Includes the saved transcript and board on a restored native room.
    engine.control(_embeddedRoom, {'type': 'resync'});

    // Poll rather than call back: a callback from a Rust worker thread has to
    // be marshalled to this isolate, and getting that wrong is a crash.
    _pump = Timer.periodic(const Duration(milliseconds: 250), (_) {
      final room = _embeddedRoom;
      if (room == 0) return;
      for (final message in engine.poll(room)) {
        if (message['type'] == 'workspace_call') {
          unawaited(_executeWorkspace(message));
        } else if (message['type'] == 'workspace_cancel') {
          unawaited(_workspaceCalls[message['id']]?.cancelOperation());
        } else {
          client.deliver(message);
        }
      }
    });
    _renewalTimer = Timer.periodic(const Duration(minutes: 1), (_) {
      if (_store?.isActive != true) return;
      unawaited(
        _refreshProviderCredentials().catchError((Object e) {
          error = e.toString();
          _client?.pause(reason: 'Sign-in needs attention');
          if (!_disposed) notifyListeners();
        }),
      );
    });
  }

  Future<void> _refreshProviderCredentials() async {
    if (_refreshingCredentials != null) return _refreshingCredentials;
    final future = _updateProviderCredentials();
    _refreshingCredentials = future;
    try {
      await future;
    } finally {
      _refreshingCredentials = null;
    }
  }

  Future<void> _updateProviderCredentials() async {
    final credentials = ModelCatalog.credentials;
    final handle = _embeddedRoom;
    if (credentials == null || handle == 0) return;
    final environment = Map<String, String>.of(_providerEnvironment);
    for (final provider in ProviderKind.values.where(
      (p) => p.signsInOnDevice,
    )) {
      final key = credentialEnvVar(provider);
      if (!environment.containsKey(key)) continue;
      final token = await credentials.freshToken(provider);
      if (token == null) {
        throw EngineStartError(
          'Sign in to ${provider.label} again from Accounts, then resume this room.',
        );
      }
      environment[key] = token;
    }
    if (_disposed ||
        handle != _embeddedRoom ||
        mapEquals(environment, _providerEnvironment)) {
      return;
    }
    if (_embedded?.control(handle, {
          'type': 'update_credentials',
          'environment': environment,
        }) !=
        true) {
      throw EngineStartError(
        'Could not update the room sign-in. Stop and reopen this saved room.',
      );
    }
    _providerEnvironment = environment;
  }

  Future<void> _executeWorkspace(Map<String, Object?> message) async {
    final handle = _embeddedRoom;
    _store?.noteWorkspace(
      message['agent'] as String,
      message['name'] as String,
    );
    final elapsed = Stopwatch()..start();
    Object? result;
    var failed = false;
    try {
      final connection = workspace;
      if (connection == null) throw StateError('No computer is connected.');
      _workspaceCalls[message['id'] as int] = connection;
      if (message['name'] == 'workspace_import_attachment') {
        final id = (message['arguments'] as Map)['id'];
        final files = _store!.messages
            .expand((m) => m.files)
            .where((f) => f.id == id);
        if (files.isEmpty) {
          throw StateError(
            'Only files already sent to this room can be imported.',
          );
        }
        final file = files.first;
        result = await connection.importAttachment(
          await RoomAttachments(room.id).original(file.id),
          file.name,
        );
      } else {
        result = await connection.execute(
          message['name'] as String,
          (message['arguments'] as Map).cast<String, Object?>(),
        );
      }
      if (result is Map &&
          result['exit_code'] != null &&
          result['exit_code'] != 0) {
        failed = true;
      }
    } catch (e) {
      result = e.toString();
      failed = true;
    }
    _workspaceCalls.remove(message['id']);
    _archive?.append([
      {
        'type': 'workspace_activity',
        'at': DateTime.now().millisecondsSinceEpoch,
        'agent': message['agent'], 'name': message['name'],
        'failed': failed, 'connected': workspace?.connected == true,
        'elapsed_ms': elapsed.elapsedMilliseconds,
        // Arguments, command output and credentials are deliberately excluded.
      },
    ]);
    if (!_disposed && handle != 0 && handle == _embeddedRoom) {
      _embedded?.control(handle, {
        'type': 'workspace_result',
        'id': message['id'],
        'content': result is String ? result : jsonEncode(result),
        'is_error': failed,
      });
      notifyListeners();
    }
  }

  /// Say something to the room.
  ///
  /// Shown immediately, then confirmed by the room's saved transcript. New
  /// input interrupts API waits; an active workspace operation finishes first.
  Future<bool> say(
    String author,
    String text, {
    List<ChatFile> files = const [],
    String? replyTo,
  }) async {
    final body = text.trim();
    if (body.isEmpty || !isRunning || _controlBusy) return false;
    if (_store?.isCompleted == true) {
      _controlBusy = true;
      try {
        await _prepareResume();
      } catch (e) {
        await _background(false);
        error = 'Could not continue: $e';
        if (!_disposed) notifyListeners();
        return false;
      } finally {
        _controlBusy = false;
      }
    }
    if (_disposed || !isRunning) return false;
    final embedded = _embedded;
    if (embedded != null && _embeddedRoom != 0) {
      if (!embedded.control(_embeddedRoom, {
        'type': 'send_message',
        'author': author,
        'body': body,
        'files': files.map((f) => f.id).toList(),
        'reply_to': ?replyTo,
      })) {
        return false;
      }
    } else {
      _client?.send({
        'type': 'send_message',
        'author': author,
        'body': body,
        if (files.isNotEmpty) 'files': files.map((f) => f.id).toList(),
        'reply_to': ?replyTo,
      });
    }
    _store?.addPending(author, body, files: files);
    notifyListeners();
    return true;
  }

  Future<Map<String, Object?>> _memoryRequest(
    String action, {
    String? id,
    bool? shared,
    String? query,
    String? targetProject,
  }) async {
    final client = _client;
    if (client == null || client.current != LinkState.connected) {
      throw StateError('Open the room before managing project memory.');
    }
    final requestId = '${room.id}-${++_memoryRequestSequence}';
    final response = Completer<Map<String, Object?>>();
    _memoryRequests[requestId] = response;
    final request = <String, Object?>{
      'request_id': requestId,
      'action': action,
    };
    if (id != null) request['id'] = id;
    if (shared != null) request['shared'] = shared;
    if (query != null) request['query'] = query;
    if (targetProject != null) request['target_project'] = targetProject;
    client.send({'type': 'memory', 'request': request});
    try {
      final result = await response.future.timeout(const Duration(seconds: 22));
      if (result['ok'] != true) {
        throw StateError(
          result['error'] as String? ?? 'Project memory request failed.',
        );
      }
      return result;
    } finally {
      _memoryRequests.remove(requestId);
    }
  }

  Future<List<Map<String, Object?>>> listProjectMemory() async =>
      ((await _memoryRequest('list'))['entries'] as List)
          .cast<Map>()
          .map((entry) => entry.cast<String, Object?>())
          .toList(growable: false);

  Future<List<Map<String, Object?>>> searchProjectMemory(String query) async =>
      ((await _memoryRequest('search', query: query))['hits'] as List)
          .cast<Map>()
          .map((hit) => hit.cast<String, Object?>())
          .toList(growable: false);

  Future<List<Map<String, Object?>>> setProjectMemoryShared(
    String id,
    bool shared,
  ) async =>
      ((await _memoryRequest('set_shared', id: id, shared: shared))['entries']
              as List)
          .cast<Map>()
          .map((entry) => entry.cast<String, Object?>())
          .toList(growable: false);

  Future<List<Map<String, Object?>>> deleteProjectMemory(String id) async =>
      ((await _memoryRequest('delete', id: id))['entries'] as List)
          .cast<Map>()
          .map((entry) => entry.cast<String, Object?>())
          .toList(growable: false);

  Future<List<Map<String, Object?>>> shareProjectMemory(
    String id,
    String targetProject,
  ) async =>
      ((await _memoryRequest(
                'share_to_project',
                id: id,
                targetProject: targetProject,
              ))['entries']
              as List)
          .cast<Map>()
          .map((entry) => entry.cast<String, Object?>())
          .toList(growable: false);

  /// Pause or resume, depending on where the room is.
  Future<void> reconnectWorkspace() async {
    if (_controlBusy || _disposed || _workspaceCalls.isNotEmpty) return;
    _controlBusy = true;
    try {
      await workspace?.close();
      final connection = WorkspaceConnection(room.workspace.copy(), room.id);
      workspace = connection;
      await connection.connect(trust: trustHost);
      if (_disposed) {
        await connection.close();
        return;
      }
      error = null;
    } catch (e) {
      error = 'Reconnect failed: $e';
    } finally {
      _controlBusy = false;
      if (!_disposed) notifyListeners();
    }
  }

  Future<void> _prepareResume() async {
    await _background(true);
    if (!runsInProcess) return;
    await _refreshProviderCredentials();
    if (workspace?.connected == true) return;
    await workspace?.close();
    final connection = WorkspaceConnection(room.workspace.copy(), room.id);
    workspace = connection;
    await connection.connect(trust: trustHost);
    if (_disposed) {
      await connection.close();
      throw StateError('Room closed before continuing.');
    }
  }

  Future<void> togglePause() async {
    final store = _store;
    if (store == null || _controlBusy || _disposed) return;
    _controlBusy = true;
    try {
      if (store.canResume) await _prepareResume();
      error = null;
      store.togglePause();
    } catch (e) {
      await _background(false);
      error = 'Reconnect failed: $e';
      if (!_disposed) notifyListeners();
    } finally {
      _controlBusy = false;
    }
  }

  Future<void> _teardown({bool stopServer = false}) async {
    _renewalTimer?.cancel();
    _renewalTimer = null;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    await _linkWatch?.cancel();
    _linkWatch = null;
    _providerEnvironment = {};
    _pump?.cancel();
    _pump = null;
    await workspace?.close();
    workspace = null;
    final embedded = _embedded;
    if (embedded != null && _embeddedRoom != 0) {
      embedded.closeRoom(_embeddedRoom);
      _embeddedRoom = 0;
    }
    _embedded = null;
    _store?.removeListener(_changed);
    await _archiveSubscription?.cancel();
    _archiveSubscription = null;
    for (final request in _memoryRequests.values) {
      if (!request.isCompleted) {
        request.completeError(StateError('Room connection closed.'));
      }
    }
    _memoryRequests.clear();
    await _archive?.close();
    _archive = null;
    await _client?.close();
    _client = null;
    final server = _server;
    if (stopServer) {
      await server?.stop();
    } else {
      await server?.detach();
    }
    _server = null;
    await _background(false);
  }

  /// Stop explicitly; navigation only detaches the screen.
  Future<void> stop() async {
    if (isRunning && _store?.isActive == true) {
      _client?.pause(reason: 'Stopped by the owner');
      for (var i = 0; i < 40 && _store?.isActive == true; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
    }
    ++_generation;
    await _teardown(stopServer: true);
    _to(RoomLaunch.closed);
  }

  @override
  void dispose() {
    _disposed = true;
    ++_generation;
    _store?.removeListener(_changed);
    unawaited(_teardown().whenComplete(() => _store?.dispose()));
    super.dispose();
  }
}

/// Project memory is shared only for an existing local folder. A remote or
/// not-yet-created path stays room-scoped until the owner selects a project.
Future<String?> _projectScope(String path) async {
  if (path.trim().isEmpty) return null;
  final directory = Directory(expandHome(path));
  if (!await directory.exists()) return null;
  try {
    return 'project:${await directory.resolveSymbolicLinks()}';
  } on FileSystemException {
    return null;
  }
}

// Only strings cross this boundary. Reading a large saved room stays off the UI.
Future<int> _openEmbeddedRoom(String toml, Map<String, String> environment) =>
    Isolate.run(() => EmbeddedEngine.load().openRoom(toml, environment));
