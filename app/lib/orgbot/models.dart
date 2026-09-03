// ignore_for_file: prefer_interpolation_to_compose_strings

// OrgBot data model: bots you create and rooms (group chats of those bots).
library;

import 'dart:math';

import '../engine/agent_cli.dart';

String genId(String prefix) {
  final rnd = Random.secure();
  return prefix +
      '_' +
      List.generate(8, (_) => rnd.nextInt(256))
          .map((b) => b.toRadixString(16).padLeft(2, '0'))
          .join();
}

/// Which brain a bot uses.
///
/// The three subscription backends run this machine's own agent CLI, so they
/// inherit its sign-in — no key is asked for or stored. Only a raw API
/// endpoint needs one.
enum ProviderKind {
  anthropic('anthropic', 'Anthropic (Claude)'),
  codex('codex', 'Codex / ChatGPT'),
  grok('grok', 'Grok'),
  deepseek('deepseek', 'DeepSeek'),
  scripted('scripted', 'Demo (no model)');

  final String key;
  final String label;
  const ProviderKind(this.key, this.label);

  static ProviderKind fromKey(String key) => values.firstWhere(
        (k) => k.key == key,
        // `openai` was the old name for the Codex backend; map it forward so
        // bots saved before the rename keep working.
        orElse: () => key == 'openai' ? ProviderKind.codex : ProviderKind.scripted,
      );

  /// The agent CLI backing this provider, or null when it talks to an API.
  CliKind? get cli {
    switch (this) {
      case ProviderKind.anthropic:
        return CliKind.claude;
      case ProviderKind.codex:
        return CliKind.codex;
      case ProviderKind.grok:
        return CliKind.grok;
      case ProviderKind.deepseek:
      case ProviderKind.scripted:
        return null;
    }
  }

  /// Whether this backend signs in through a CLI rather than taking a key.
  bool get usesSignIn => cli != null;

  /// Whether an API key must be supplied. Only a raw endpoint needs one.
  bool get needsApiKey => this == ProviderKind.deepseek;
}

class BotDef {
  final String id;
  String name;
  String handle;
  String persona;
  ProviderKind provider;
  String model;
  String apiKey;

  BotDef({
    String? id,
    required this.name,
    String? handle,
    this.persona = '',
    this.provider = ProviderKind.anthropic,
    this.model = '',
    this.apiKey = '',
  })  : id = id ?? genId('bot'),
        handle = handle == null || handle.isEmpty ? '' : handle;

  /// Whether this bot still needs a key from the user.
  bool get needsKey => provider.needsApiKey && apiKey.trim().isEmpty;

  Map<String, Object?> toJson() => {
        'id': id,
        'name': name,
        'handle': handle,
        'persona': persona,
        'provider': provider.key,
        'model': model,
        'apiKey': apiKey,
      };

  factory BotDef.fromJson(Map<String, Object?> json) => BotDef(
        id: json['id'] as String,
        name: json['name'] as String? ?? '',
        handle: json['handle'] as String?,
        persona: json['persona'] as String? ?? '',
        provider: ProviderKind.fromKey(json['provider'] as String? ?? ''),
        model: json['model'] as String? ?? '',
        apiKey: json['apiKey'] as String? ?? '',
      );
}

class RoomSlot {
  final String botId;
  String role; // writer | coordinator | reviewer
  RoomSlot(this.botId, this.role);
  Map<String, Object?> toJson() => {'bot_id': botId, 'roles': [role]};
  factory RoomSlot.fromJson(Map<String, Object?> json) =>
      RoomSlot(json['bot_id'] as String, (json['roles'] as List? ?? []).isNotEmpty
          ? (json['roles'] as List).first as String
          : 'writer');
}

class RoomDef {
  final String id;
  String name;
  String goal;
  String path; // unused on-device; kept for desktop parity
  List<String> tasks;
  List<RoomSlot> bots;

  RoomDef({
    String? id,
    required this.name,
    this.goal = '',
    this.path = '',
    List<String>? tasks,
    List<RoomSlot>? bots,
  })  : id = id ?? genId('room'),
        tasks = tasks ?? [],
        bots = bots ?? [];

  Map<String, Object?> toJson() => {
        'id': id,
        'name': name,
        'goal': goal,
        'path': path,
        'tasks': tasks,
        'bots': bots.map((b) => b.toJson()).toList(),
      };

  factory RoomDef.fromJson(Map<String, Object?> json) => RoomDef(
        id: json['id'] as String,
        name: json['name'] as String? ?? '',
        goal: json['goal'] as String? ?? '',
        path: json['path'] as String? ?? '',
        tasks: (json['tasks'] as List? ?? []).cast<String>(),
        bots: (json['bots'] as List? ?? [])
            .map((b) => RoomSlot.fromJson((b as Map).cast<String, Object?>()))
            .toList(),
      );
}
