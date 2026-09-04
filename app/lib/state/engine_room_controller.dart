// Runs a room on the real engine and exposes it to the UI.
//
// Owns the lifecycle the screen should not have to think about: generate the
// room file, start `orgbot serve`, connect the WebSocket, and tear all of it
// down again when the screen closes.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:async';

import 'package:flutter/foundation.dart';

import 'dart:io';

import '../engine/core_server.dart';
import '../engine/embedded_engine.dart';
import '../net/room_client.dart';
import '../orgbot/library.dart';
import '../orgbot/models.dart';
import '../orgbot/model_catalog.dart';
import '../orgbot/room_check.dart';
import '../orgbot/room_file.dart';
import 'room_store.dart';

/// How far along starting a room is.
enum RoomLaunch { idle, generating, starting, connecting, running, failed, closed }

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
  RoomLink? _client;
  RoomStore? _store;

  /// Set when the room runs inside this process rather than as a subprocess.
  EmbeddedEngine? _embedded;
  int _embeddedRoom = 0;
  Timer? _pump;

  /// Whether a room here runs in-process.
  ///
  /// A phone cannot spawn a subprocess, so it loads the engine as a library
  /// instead. Same engine either way — only how it is started differs.
  static bool get runsInProcess => Platform.isAndroid || Platform.isIOS;

  /// The live room state, once connected.
  RoomStore? get store => _store;

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
        return 'Running';
      case RoomLaunch.failed:
        return error ?? 'Failed to start';
      case RoomLaunch.closed:
        return 'Closed';
    }
  }

  void _to(RoomLaunch next) {
    phase = next;
    notifyListeners();
  }

  /// Generate the room file, start the engine, and connect to it.
  Future<void> start() async {
    if (phase != RoomLaunch.idle && phase != RoomLaunch.failed) return;
    error = null;
    _to(RoomLaunch.generating);

    try {
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

      final file = buildRoomFile(room, bots,
          credentials: ModelCatalog.credentials);
      skipped = file.skipped;
      if (file.toml.contains('[[bots]]') == false) {
        throw EngineStartError(skipped.isEmpty
            ? 'No usable bots in this room.'
            : 'No usable bots: ${skipped.join('; ')}');
      }

      _to(RoomLaunch.starting);
      if (runsInProcess) {
        await _startEmbedded(file);
      } else {
        await _startSubprocess(file);
      }
      _to(RoomLaunch.running);
    } catch (e) {
      error = e.toString();
      await _teardown();
      _to(RoomLaunch.failed);
    }
  }

  /// Run the room as a child process and talk to it over a socket.
  Future<void> _startSubprocess(RoomFile file) async {
    final server = await startRoom(file);
    _server = server;

    _to(RoomLaunch.connecting);
    final client = RoomClient();
    _client = client;
    final store = RoomStore(client);
    _store = store;
    store.addListener(notifyListeners);

    await client.connect(server.url);
    if (client.current == LinkState.error) {
      throw EngineStartError('Connected to the engine but the socket failed.');
    }
  }

  /// Run the room inside this process, through the engine library.
  Future<void> _startEmbedded(RoomFile file) async {
    final engine = EmbeddedEngine.load();
    _embedded = engine;
    _embeddedRoom = engine.openRoom(file.toml, file.environment);

    _to(RoomLaunch.connecting);
    // The store is fed the same wire messages the socket would carry, so
    // nothing above it knows which transport it got.
    final client = LocalRoomClient();
    _client = client;
    final store = RoomStore(client);
    _store = store;
    store.addListener(notifyListeners);

    final snapshot = engine.snapshot(_embeddedRoom);
    if (snapshot != null) client.deliver(snapshot);

    // Poll rather than call back: a callback from a Rust worker thread has to
    // be marshalled to this isolate, and getting that wrong is a crash.
    _pump = Timer.periodic(const Duration(milliseconds: 250), (_) {
      final room = _embeddedRoom;
      if (room == 0) return;
      for (final message in engine.poll(room)) {
        client.deliver(message);
      }
    });
  }

  /// Say something to the room.
  ///
  /// Shown straight away rather than waiting for the room to echo it back: a
  /// bot turn can run for a minute and messages are applied between turns, so
  /// the round trip is long enough to look like the app lost what you typed.
  void say(String author, String text) {
    final body = text.trim();
    if (body.isEmpty) return;
    _store?.addPending(author, body);
    final embedded = _embedded;
    if (embedded != null && _embeddedRoom != 0) {
      embedded.say(_embeddedRoom, author, body);
    } else {
      _client?.sendMessage(author, body);
    }
    notifyListeners();
  }

  /// Pause or resume, depending on where the room is.
  void togglePause() {
    final store = _store;
    if (store == null) return;
    store.togglePause();
  }

  Future<void> _teardown() async {
    _pump?.cancel();
    _pump = null;
    final embedded = _embedded;
    if (embedded != null && _embeddedRoom != 0) {
      embedded.closeRoom(_embeddedRoom);
      _embeddedRoom = 0;
    }
    _embedded = null;
    _store?.removeListener(notifyListeners);
    await _client?.close();
    _client = null;
    await _server?.stop();
    _server = null;
  }

  @override
  Future<void> dispose() async {
    await _teardown();
    super.dispose();
  }
}
