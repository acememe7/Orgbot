// ignore_for_file: prefer_interpolation_to_compose_strings

// OrgBot data model: bots you create and rooms (group chats of those bots).
library;

import 'dart:math';
import '../workspace/workspace.dart';

import '../engine/agent_cli.dart';
import 'roles.dart';

String genId(String prefix) {
  final rnd = Random.secure();
  return prefix +
      '_' +
      List.generate(
        8,
        (_) => rnd.nextInt(256),
      ).map((b) => b.toRadixString(16).padLeft(2, '0')).join();
}

/// Which brain a bot uses.
///
/// CLI-backed providers keep their provider configuration in the local CLI.
/// OrgBots does not store an API key for them; API endpoints need a key stored
/// by the app. Provider charges depend on the CLI and provider configuration.
enum ProviderKind {
  anthropic('anthropic', 'Anthropic (Claude)'),
  codex('codex', 'Codex / ChatGPT'),
  grok('grok', 'Grok'),
  opencode('opencode', 'OpenCode'),
  antigravity('antigravity', 'Antigravity'),
  deepseek('deepseek', 'DeepSeek'),
  ollama('ollama', 'Ollama Cloud'),
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
      case ProviderKind.opencode:
        return CliKind.opencode;
      case ProviderKind.antigravity:
        return CliKind.antigravity;
      case ProviderKind.deepseek:
      case ProviderKind.ollama:
      case ProviderKind.scripted:
        return null;
    }
  }

  /// Whether this backend signs in through a CLI rather than taking a key.
  bool get usesSignIn => cli != null;

  /// Whether this CLI requires the user to supply a model identifier.
  bool get requiresExplicitModel =>
      this == ProviderKind.opencode || this == ProviderKind.antigravity;

  /// Whether an API key must be supplied. Only a raw endpoint needs one.
  bool get needsApiKey =>
      this == ProviderKind.deepseek || this == ProviderKind.ollama;

  /// API origin; the engine adds /v1/chat/completions.
  String get apiBaseUrl => switch (this) {
    ProviderKind.deepseek => 'https://api.deepseek.com',
    ProviderKind.ollama => 'https://ollama.com',
    _ => throw StateError('$label does not use a saved API key'),
  };

  /// Whether a device with no agent CLIs can sign in to this itself.
  ///
  /// Codex and Grok publish device-code flows; Anthropic does not, so a
  /// Claude subscription can only be used where its CLI runs.
  bool get signsInOnDevice =>
      this == ProviderKind.codex || this == ProviderKind.grok;
}

class BotDef {
  final String id;
  String name;
  String handle;
  String persona;
  ProviderKind provider;
  String model;
  String apiKey;

  /// How hard an agent CLI reasons (`low` … `max`); empty keeps the CLI's own
  /// default. Only levels the bot's CLI offers are sent to it.
  String effort;

  /// The job this bot holds. A property of the bot, not of a room — the same
  /// reviewer reviews wherever it is seated.
  String role;
  List<String> additionalRoles;
  List<String> get roles => {role, ...additionalRoles}.toList();

  /// Permissions overriding the role's defaults, when you have deliberately
  /// changed them. Null means "whatever the role says".
  RolePowers? powers;

  BotDef({
    String? id,
    required this.name,
    String? handle,
    this.persona = '',
    this.provider = ProviderKind.anthropic,
    this.model = '',
    this.apiKey = '',
    this.effort = '',
    this.role = Roles.defaultKey,
    this.additionalRoles = const [],
    this.powers,
  }) : id = id ?? genId('bot'),
       handle = handle == null || handle.isEmpty ? '' : handle;

  /// Whether this bot still needs a key from the user.
  bool get needsKey => provider.needsApiKey && apiKey.trim().isEmpty;

  /// The permissions actually in force: the override if one was set, else the
  /// role's defaults.
  RolePowers get effectivePowers => powers ?? Roles.byKey(role).powers;

  /// The role definition this bot holds.
  RoleDef get roleDef => Roles.byKey(role);

  Map<String, Object?> toJson() => {
    'id': id,
    'name': name,
    'handle': handle,
    'persona': persona,
    'provider': provider.key,
    'model': model,
    'apiKey': apiKey,
    if (effort.isNotEmpty) 'effort': effort,
    'role': role,
    'additional_roles': additionalRoles,
    'powers': powers?.toJson(),
  };

  factory BotDef.fromJson(Map<String, Object?> json) => BotDef(
    id: json['id'] as String,
    name: json['name'] as String? ?? '',
    handle: json['handle'] as String?,
    persona: json['persona'] as String? ?? '',
    provider: ProviderKind.fromKey(json['provider'] as String? ?? ''),
    model: json['model'] as String? ?? '',
    apiKey: json['apiKey'] as String? ?? '',
    effort: json['effort'] as String? ?? '',
    role: json['role'] as String? ?? Roles.defaultKey,
    additionalRoles: (json['additional_roles'] as List? ?? [])
        .whereType<String>()
        .toList(),
    powers: json['powers'] is Map
        ? RolePowers.fromJson((json['powers'] as Map).cast<String, Object?>())
        : null,
  );
}

/// A bot seated in a room. The role is no longer here: it belongs to the bot,
/// so the same reviewer reviews in every room it joins.
class RoomSlot {
  final String botId;
  RoomSlot(this.botId);
  Map<String, Object?> toJson() => {'bot_id': botId};
  factory RoomSlot.fromJson(Map<String, Object?> json) =>
      RoomSlot(json['bot_id'] as String);
}

/// The turn budget a new room starts with.
///
/// Higher than the old fixed 500: a room with a dozen bots working a real
/// goal spends that in about a day, and every exhaustion is a stop that waits
/// on a person. Still finite, because it is the only thing standing between a
/// misbehaving room and an unbounded bill.
const int defaultMaxTurns = 2000;

/// Desktop sessions continue until completion or an explicit pause.
const int desktopDefaultMaxTurns = 0;
const List<int> desktopTurnBudgetChoices = [0, 100, 500, 2000];

/// Turn budgets offered in the UI. `0` means no limit.
const List<int> turnBudgetChoices = [500, 2000, 5000, 20000, 0];

/// How many bots work at the same time in a new room.
///
/// A turn is almost all waiting on a model, so this is what decides how much
/// of the roster is actually working: at one, a room of twenty bots has
/// nineteen idle at every moment and seating more cannot make the work finish
/// sooner.
///
/// Two, not four. Concurrency is a rate multiplier as much as a speed-up — a
/// room drawing four at a time spends roughly four times as much per hour as
/// one at a time — and a default that quadruples someone's consumption is a
/// decision made on their behalf with their allowance. Four was the default
/// for a day and took a weekly subscription limit with it. Two is a real
/// improvement over taking turns one at a time and leaves the larger step as
/// something chosen, by someone who can see what the room has drawn.
const int defaultParallelTurns = 2;

const int desktopDefaultParallelTurns = 4;

/// Concurrency settings offered in the UI.
///
/// Stops at eight deliberately. Past that the limit stops being the room and
/// starts being the providers, and a room that spends its turns collecting
/// rate-limit errors is slower than one that waited its turn.
const List<int> parallelTurnChoices = [1, 2, 4, 6, 8];

class RoomDef {
  final String id;
  String name;
  String goal;
  String path; // Desktop CLI working directory.
  String memory;
  WorkspaceConfig workspace;
  List<String> tasks;
  List<RoomSlot> bots;

  /// How many model turns the room may take before it stops and asks you.
  ///
  /// A spend control, not a deadline: hitting it pauses the room rather than
  /// ending it, and resuming grants another budget. The default was 500 and
  /// was not settable, which meant a long-running room stopped roughly once a
  /// day and waited — sometimes for hours — for someone to notice.
  int maxTurns;

  /// How many bots may be taking a turn at the same moment.
  ///
  /// Raising this does not risk the board. Two bots reaching for the same task
  /// is settled by the engine — one gets it, the other is told it was taken —
  /// so what this actually spends is provider rate limit, not correctness.
  int parallelTurns;

  RoomDef({
    String? id,
    required this.name,
    this.goal = '',
    this.path = '',
    this.memory = '',
    this.maxTurns = defaultMaxTurns,
    this.parallelTurns = defaultParallelTurns,
    WorkspaceConfig? workspace,
    List<String>? tasks,
    List<RoomSlot>? bots,
  }) : id = id ?? genId('room'),
       workspace = workspace ?? WorkspaceConfig(),
       tasks = tasks ?? [],
       bots = bots ?? [];

  Map<String, Object?> toJson() => {
    'id': id,
    'name': name,
    'goal': goal,
    'path': path,
    'memory': memory,
    'workspace': workspace.toJson(),
    'tasks': tasks,
    'max_turns': maxTurns,
    'parallel_turns': parallelTurns,
    'bots': bots.map((b) => b.toJson()).toList(),
  };

  factory RoomDef.fromJson(Map<String, Object?> json) => RoomDef(
    id: json['id'] as String,
    name: json['name'] as String? ?? '',
    goal: json['goal'] as String? ?? '',
    path: json['path'] as String? ?? '',
    memory: json['memory'] as String? ?? '',
    workspace: json['workspace'] is Map
        ? WorkspaceConfig.fromJson(
            (json['workspace'] as Map).cast<String, Object?>(),
          )
        : null,
    tasks: (json['tasks'] as List? ?? []).cast<String>(),
    maxTurns: (json['max_turns'] as num?)?.toInt() ?? defaultMaxTurns,
    parallelTurns:
        (json['parallel_turns'] as num?)?.toInt() ?? defaultParallelTurns,
    bots: (json['bots'] as List? ?? [])
        .map((b) => RoomSlot.fromJson((b as Map).cast<String, Object?>()))
        .toList(),
  );
}
