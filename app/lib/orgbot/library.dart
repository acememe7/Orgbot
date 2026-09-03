// The persisted bot library and rooms for the phone app.
library;

import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'models.dart';

/// Where the library persists. SharedPreferences on device; injectable in
/// tests.
abstract class Saver {
  Future<String?> load();
  Future<void> save(String json);
}

class PrefsSaver implements Saver {
  static const _key = 'orgbot_library_v1';
  @override
  Future<String?> load() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_key);
  }

  @override
  Future<void> save(String json) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_key, json);
  }
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
  final Saver saver;
  List<BotDef> bots = [];
  List<RoomDef> rooms = [];
  bool loaded = false;

  Library({Saver? saver}) : saver = saver ?? PrefsSaver();

  Future<void> load() async {
    final raw = await saver.load();
    if (raw != null && raw.isNotEmpty) {
      try {
        final data = jsonDecode(raw) as Map<String, Object?>;
        bots = (data['bots'] as List? ?? [])
            .map((b) => BotDef.fromJson((b as Map).cast<String, Object?>()))
            .toList();
        rooms = (data['rooms'] as List? ?? [])
            .map((r) => RoomDef.fromJson((r as Map).cast<String, Object?>()))
            .toList();
      } catch (_) {
        bots = [];
        rooms = [];
      }
    }
    loaded = true;
    notifyListeners();
  }

  Future<void> _persist() async {
    await saver.save(jsonEncode({
      'bots': bots.map((b) => b.toJson()).toList(),
      'rooms': rooms.map((r) => r.toJson()).toList(),
    }));
    notifyListeners();
  }

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
    rooms.removeWhere((r) => r.bots.any((s) => s.botId == id));
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
    rooms.removeWhere((r) => r.id == id);
    _persist();
  }
}
