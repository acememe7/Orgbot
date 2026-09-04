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

import '../engine/core_server.dart';
import '../net/room_client.dart';
import '../orgbot/library.dart';
import '../orgbot/models.dart';
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
  RoomClient? _client;
  RoomStore? _store;

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

      final file = buildRoomFile(room, bots);
      skipped = file.skipped;
      if (file.toml.contains('[[bots]]') == false) {
        throw EngineStartError(skipped.isEmpty
            ? 'No usable bots in this room.'
            : 'No usable bots: ${skipped.join('; ')}');
      }

      _to(RoomLaunch.starting);
      final server = await startRoom(file);
      _server = server;

      _to(RoomLaunch.connecting);
      final client = RoomClient();
      _client = client;
      final store = RoomStore(client);
      _store = store;
      // Repaint the screen whenever room state changes.
      store.addListener(notifyListeners);

      await client.connect(server.url);
      if (client.current == LinkState.error) {
        throw EngineStartError('Connected to the engine but the socket failed.');
      }
      _to(RoomLaunch.running);
    } catch (e) {
      error = e.toString();
      await _teardown();
      _to(RoomLaunch.failed);
    }
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
    _client?.sendMessage(author, body);
    notifyListeners();
  }

  /// Pause or resume, depending on where the room is.
  void togglePause() {
    final store = _store;
    if (store == null) return;
    store.togglePause();
  }

  Future<void> _teardown() async {
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
