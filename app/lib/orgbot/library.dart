// The persisted bot library and rooms for the phone app.
library;

import 'dart:convert';
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'models.dart';
import 'credentials.dart';
import '../state/engine_room_controller.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// Where the library persists. Encrypted platform storage on device; injectable
/// in tests.
abstract class Saver {
  Future<String?> load();
  Future<void> save(String json);
}

class PrefsSaver implements Saver {
  static const _key = 'orgbot_library_v1';
  final FlutterSecureStorage secure;
  PrefsSaver({FlutterSecureStorage? secure})
    : secure = secure ?? const FlutterSecureStorage();

  @override
  Future<String?> load() async {
    final saved = await secure.read(key: _key);
    final prefs = await SharedPreferences.getInstance();
    final legacy = prefs.getString(_key);
    if (saved != null) {
      if (legacy != null) await prefs.remove(_key);
      return saved;
    }
    if (legacy == null) return null;
    // Commit to the keystore before removing the only existing copy.
    await secure.write(key: _key, value: legacy);
    await prefs.remove(_key);
    return legacy;
  }

  @override
  Future<void> save(String json) => secure.write(key: _key, value: json);
}

class MemorySaver implements Saver {
  String? data;
  @override
  Future<String?> load() async => data;
  @override
  Future<void> save(String json) async => data = json;
}

/// Holds every bot and room the user created.
class Library extends ChangeNotifier {
  bool _disposed = false;
  final Map<String, EngineRoomController> sessions = {};

  EngineRoomController session(RoomDef room) =>
      sessions.putIfAbsent(room.id, () {
        final session = EngineRoomController(room: room, library: this);
        session.addListener(notifyListeners);
        return session;
      });

  Future<void> stopRoom(String id) async {
    final session = sessions.remove(id);
    if (session != null) {
      session.removeListener(notifyListeners);
      await session.stop();
      session.dispose();
    }
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    for (final session in sessions.values) {
      session.removeListener(notifyListeners);
      session.dispose();
    }
    sessions.clear();
    super.dispose();
  }

  final Saver saver;
  List<BotDef> bots = [];
  List<RoomDef> rooms = [];
  String userName = 'You';
  String userProfile = '';
  bool loaded = false;
  String? error;
  Future<void> _saving = Future.value();
  Future<void>? _loading;

  Library({Saver? saver}) : saver = saver ?? PrefsSaver();

  /// Upgrade the old per-bot key when there is one unambiguous shared account.
  Future<void> shareExistingKeys(Credentials credentials) async {
    if (!loaded) return;
    await credentials.load();
    final provider = ProviderKind.deepseek;
    final keys = bots
        .where((b) => b.provider == provider)
        .map((b) => b.apiKey.trim())
        .where((k) => k.isNotEmpty)
        .toSet();
    if (!credentials.isSignedIn(provider) && keys.length == 1) {
      await credentials.saveApiKey(provider, keys.single);
    }
    final shared = credentials.forProvider(provider)?.token;
    if (shared == null) return;
    var changed = false;
    for (final bot in bots.where(
      (b) => b.provider == provider && b.apiKey.trim() == shared,
    )) {
      bot.apiKey = '';
      changed = true;
    }
    if (changed) await save();
  }

  Future<void> load() {
    if (loaded) return Future.value();
    return _loading ??= _load().whenComplete(() => _loading = null);
  }

  void reportError(String message) {
    error = message;
    if (!_disposed) notifyListeners();
  }

  Future<void> _load() async {
    String? raw;
    try {
      raw = await saver.load();
    } catch (_) {
      error =
          'Could not unlock your saved bots and rooms. Try opening the app again.';
      if (!_disposed) notifyListeners();
      return;
    }
    if (raw != null && raw.isNotEmpty) {
      try {
        final data = jsonDecode(raw) as Map<String, Object?>;
        final user = data['user'] as Map?;
        userName = (user?['name'] as String?)?.trim() ?? 'You';
        if (userName.isEmpty) userName = 'You';
        userProfile = user?['profile'] as String? ?? '';
        bots = (data['bots'] as List? ?? [])
            .map((b) => BotDef.fromJson((b as Map).cast<String, Object?>()))
            .toList();
        rooms = (data['rooms'] as List? ?? [])
            .map((r) => RoomDef.fromJson((r as Map).cast<String, Object?>()))
            .toList();
      } catch (_) {
        error =
            'Your saved library could not be read. It has been kept intact.';
        if (!_disposed) notifyListeners();
        return;
      }
    }
    loaded = true;
    error = null;
    if (!_disposed) notifyListeners();
  }

  Future<void> _persist() {
    final json = jsonEncode({
      'bots': bots.map((b) => b.toJson()).toList(),
      'rooms': rooms.map((r) => r.toJson()).toList(),
      'user': {'name': userName, 'profile': userProfile},
    });
    if (!_disposed) notifyListeners();
    _saving = _saving
        .then((_) => saver.save(json))
        .then((_) {
          error = null;
          if (!_disposed) notifyListeners();
        })
        .catchError((Object _) {
          error =
              'Changes could not be saved securely. Keep the app open and retry.';
          if (!_disposed) notifyListeners();
        });
    return _saving;
  }

  Future<void> save() => _persist();

  BotDef addBot(BotDef bot) {
    bots.add(bot);
    _persist();
    return bot;
  }

  void updateBot(BotDef bot) {
    final i = bots.indexWhere((b) => b.id == bot.id);
    if (i >= 0) bots[i] = bot;
    _persist();
  }

  void removeBot(String id) {
    bots.removeWhere((b) => b.id == id);
    for (final room in rooms) {
      room.bots.removeWhere((slot) => slot.botId == id);
    }
    _persist();
  }

  BotDef? botById(String id) {
    for (final b in bots) {
      if (b.id == id) return b;
    }
    return null;
  }

  RoomDef addRoom(RoomDef room) {
    rooms.add(room);
    _persist();
    return room;
  }

  void removeRoom(String id) {
    unawaited(stopRoom(id));
    rooms.removeWhere((r) => r.id == id);
    _persist();
  }
}
