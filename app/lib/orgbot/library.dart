// The persisted bot library and rooms for the phone app.
library;

import 'dart:convert';
import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'models.dart';
import 'credentials.dart';
import '../protocol/models.dart' show ChatMessage;
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
  static const _specialistCoordinationVersionKey =
      'specialist_coordination_migration_version';
  static const _specialistCoordinationVersion = 1;

  bool _disposed = false;
  final Map<String, EngineRoomController> sessions = {};
  final Map<String, VoidCallback> _sessionListeners = {};
  final Map<String, _SessionSummary> _sessionSummaries = {};

  EngineRoomController session(RoomDef room) =>
      sessions.putIfAbsent(room.id, () {
        final session = EngineRoomController(room: room, library: this);
        _sessionSummaries[room.id] = _summary(session);
        void changed() {
          final next = _summary(session);
          if (_sessionSummaries[room.id] == next) return;
          _sessionSummaries[room.id] = next;
          if (!_disposed) notifyListeners();
        }

        _sessionListeners[room.id] = changed;
        session.addListener(changed);
        return session;
      });

  static _SessionSummary _summary(EngineRoomController session) {
    final messages = session.store?.messages ?? const <ChatMessage>[];
    final latest = messages.isEmpty ? null : messages.last;
    return (
      phase: session.phase,
      isActive: session.store?.isActive ?? false,
      runStateLabel: session.store?.runStateLabel ?? '',
      error: session.error,
      storeError: session.store?.lastError,
      latestMessageId: latest?.id,
      latestMessageBody: latest?.body,
    );
  }

  Future<void> stopRoom(String id) async {
    final session = sessions.remove(id);
    if (session != null) {
      final listener = _sessionListeners.remove(id);
      if (listener != null) session.removeListener(listener);
      _sessionSummaries.remove(id);
      await session.stop();
      session.dispose();
    }
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    for (final entry in sessions.entries) {
      final listener = _sessionListeners[entry.key];
      if (listener != null) entry.value.removeListener(listener);
      entry.value.dispose();
    }
    sessions.clear();
    _sessionListeners.clear();
    _sessionSummaries.clear();
    super.dispose();
  }

  final Saver saver;
  final bool _isLinux;
  int _savedSpecialistCoordinationVersion = 0;
  List<BotDef> bots = [];
  List<RoomDef> rooms = [];
  String userName = 'You';
  String userProfile = '';
  bool loaded = false;
  String? error;
  Future<void> _saving = Future.value();
  Future<void>? _loading;

  Library({Saver? saver, @visibleForTesting bool? isLinux})
    : saver = saver ?? PrefsSaver(),
      _isLinux = isLinux ?? (!kIsWeb && Platform.isLinux);

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
    Map<String, Object?>? data;
    var migrationVersion = 0;
    late _ParsedLibrary parsed;
    if (raw != null && raw.isNotEmpty) {
      try {
        final decoded = jsonDecode(raw);
        if (decoded is! Map) {
          throw const FormatException('library must be an object');
        }
        data = decoded.cast<String, Object?>();
        final savedVersion = data[_specialistCoordinationVersionKey];
        migrationVersion = savedVersion is num ? savedVersion.toInt() : 0;
        parsed = _parseLibrary(data);
      } catch (_) {
        error =
            'Your saved library could not be read. It has been kept intact.';
        if (!_disposed) notifyListeners();
        return;
      }

      if (_isLinux && migrationVersion < _specialistCoordinationVersion) {
        late final Map<String, Object?> migrated;
        late final _ParsedLibrary migratedLibrary;
        try {
          final presetJson = await rootBundle.loadString(
            'tool/specialist-team.json',
          );
          final preset = jsonDecode(presetJson);
          if (preset is! Map) {
            throw const FormatException('specialist preset must be an object');
          }
          migrated = _refreshSpecialistCoordination(
            data,
            preset.cast<String, Object?>(),
          );
          migrated[_specialistCoordinationVersionKey] =
              _specialistCoordinationVersion;
          migratedLibrary = _parseLibrary(migrated);
        } catch (_) {
          error =
              'Your saved library could not be safely updated. It has been kept intact.';
          if (!_disposed) notifyListeners();
          return;
        }

        final migratedRaw = jsonEncode(migrated);
        try {
          // Save the complete raw map before exposing migrated objects in memory.
          await saver.save(migratedRaw);
        } catch (_) {
          error =
              'Your saved library could not be securely updated. The original is still intact; keep the app open and retry.';
          if (!_disposed) notifyListeners();
          return;
        }
        data = migrated;
        parsed = migratedLibrary;
        migrationVersion = _specialistCoordinationVersion;
      }
    } else {
      parsed = _parseLibrary(null);
    }

    userName = parsed.userName;
    userProfile = parsed.userProfile;
    bots = parsed.bots;
    rooms = parsed.rooms;
    _savedSpecialistCoordinationVersion = migrationVersion;
    loaded = true;
    error = null;
    if (!_disposed) notifyListeners();
  }

  Future<void> _persist() {
    final value = <String, Object?>{
      'bots': bots.map((b) => b.toJson()).toList(),
      'rooms': rooms.map((r) => r.toJson()).toList(),
      'user': {'name': userName, 'profile': userProfile},
    };
    if (_savedSpecialistCoordinationVersion > 0) {
      value[_specialistCoordinationVersionKey] =
          _savedSpecialistCoordinationVersion;
    }
    final json = jsonEncode(value);
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

typedef _SessionSummary = ({
  RoomLaunch phase,
  bool isActive,
  String runStateLabel,
  String? error,
  String? storeError,
  String? latestMessageId,
  String? latestMessageBody,
});

const _legacyManagedPersonas = <String, String>{
  'astra':
      'You are the security and code reviewer. Review security and code changes; inspect evidence, '
      'identify concrete risks, and record actionable findings. Do not implement or review unrelated '
      'design work. Review Opus-authored code/security work independently only when that boundary is '
      'material. Avoid gratuitous calls and automatic cross-review cycles.',
  'opus':
      "You own design and architecture. Produce concrete design decisions, trade-offs, and "
      "implementation guidance. Materially cross-check Astra's security/code findings and review "
      'relevant architecture or design trade-offs when needed. Avoid pro forma reviews, forced cycles, '
      'and gratuitous model calls.',
  'grok':
      'You coordinate the room: turn the goal into a small set of clear tasks, route each task to the '
      'fitting role, avoid duplicate or gratuitous model calls, and submit completed work after required '
      'checks. Use premium models only when the task needs them.',
};

const _routingStart = '<!-- orgbot-specialist-team-routing:start -->';
const _routingEnd = '<!-- orgbot-specialist-team-routing:end -->';

_ParsedLibrary _parseLibrary(Map<String, Object?>? data) {
  final user = data?['user'] as Map?;
  final savedName = (user?['name'] as String?)?.trim() ?? 'You';
  return (
    userName: savedName.isEmpty ? 'You' : savedName,
    userProfile: user?['profile'] as String? ?? '',
    bots: (data?['bots'] as List? ?? [])
        .map((bot) => BotDef.fromJson((bot as Map).cast<String, Object?>()))
        .toList(),
    rooms: (data?['rooms'] as List? ?? [])
        .map((room) => RoomDef.fromJson((room as Map).cast<String, Object?>()))
        .toList(),
  );
}

Map<String, Object?> _refreshSpecialistCoordination(
  Map<String, Object?> source,
  Map<String, Object?> preset,
) {
  if (preset['schema_version'] != 1 ||
      preset['bots'] is! List ||
      preset['routing_memory'] is! String) {
    throw const FormatException('specialist preset is invalid');
  }
  final result = (jsonDecode(jsonEncode(source)) as Map)
      .cast<String, Object?>();
  final bots = result['bots'] as List? ?? const [];
  final rooms = result['rooms'] as List? ?? const [];
  if (bots.any((bot) => bot is! Map) || rooms.any((room) => room is! Map)) {
    throw const FormatException('saved bots or rooms are invalid');
  }
  final rawBots = bots.whereType<Map>().toList();

  const managedHandles = {'astra', 'opus', 'grok'};
  final targets = (preset['bots'] as List)
      .whereType<Map>()
      .map((bot) => bot.cast<String, Object?>())
      .where((bot) => managedHandles.contains(bot['handle']))
      .toList();
  if (targets.length != managedHandles.length) {
    throw const FormatException('specialist preset is missing a managed agent');
  }

  for (final target in targets) {
    final targetId = target['id'];
    final targetHandle = target['handle'];
    final sameId = rawBots.where((bot) => bot['id'] == targetId).toList();
    final sameHandle = rawBots
        .where((bot) => bot['handle'] == targetHandle)
        .toList();
    if (sameId.length != 1 ||
        sameHandle.length != 1 ||
        !identical(sameId.single, sameHandle.single)) {
      continue;
    }
    final bot = sameId.single.cast<String, Object?>();
    if (bot['handle'] != targetHandle ||
        bot['provider'] != target['provider'] ||
        bot['model'] != target['model']) {
      continue;
    }

    final handle = targetHandle as String;
    final rawRoles = bot['additional_roles'];
    final additionalRoles = rawRoles == null
        ? <String>[]
        : rawRoles is List && rawRoles.every((role) => role is String)
        ? rawRoles.cast<String>().toList()
        : throw const FormatException('saved specialist roles are invalid');
    if (handle == 'grok') {
      additionalRoles.removeWhere((role) => role == 'coordinator');
      if (bot['role'] == 'coordinator') bot['role'] = 'support';
    } else if (!additionalRoles.contains('coordinator')) {
      additionalRoles.add('coordinator');
    }
    bot['additional_roles'] = additionalRoles;

    final effective = BotDef.fromJson(bot).effectivePowers.toJson();
    final rawPowers = bot['powers'];
    final powers = <String, Object?>{
      ...effective,
      if (rawPowers is Map) ...rawPowers.cast<String, Object?>(),
    };
    final targetPowers = target['powers'];
    if (targetPowers is! Map) {
      throw const FormatException('specialist preset powers are invalid');
    }
    powers['may_create_tasks'] = targetPowers['may_create_tasks'];
    powers['may_submit'] = targetPowers['may_submit'];
    if (handle == 'astra') powers['review_only'] = true;
    bot['powers'] = powers;

    final persona = bot['persona'];
    final oldPrefix = _legacyManagedPersonas[handle];
    final targetPersona = target['persona'];
    if (persona is String &&
        oldPrefix != null &&
        targetPersona is String &&
        persona.startsWith(oldPrefix)) {
      final newPrefix = targetPersona.split('\n\n').first;
      bot['persona'] = '$newPrefix${persona.substring(oldPrefix.length)}';
    }
  }

  final routing = preset['routing_memory'] as String;
  for (final roomValue in rooms.whereType<Map>()) {
    final room = roomValue.cast<String, Object?>();
    final memory = room['memory'];
    if (memory is! String) continue;
    final start = memory.indexOf(_routingStart);
    if (start < 0) continue;
    final end = memory.indexOf(_routingEnd, start + _routingStart.length);
    if (end < 0) continue;
    final endAfterMarker = end + _routingEnd.length;
    final block = '$_routingStart\n${routing.trim()}\n$_routingEnd';
    room['memory'] =
        '${memory.substring(0, start)}$block${memory.substring(endAfterMarker)}';
  }
  return result;
}

typedef _ParsedLibrary = ({
  String userName,
  String userProfile,
  List<BotDef> bots,
  List<RoomDef> rooms,
});
