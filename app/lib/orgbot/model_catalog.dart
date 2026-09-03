// The models a bot can be pointed at, read from this machine rather than
// written down here.
//
// A hardcoded list goes stale and is wrong the day it ships — the previous one
// offered "gpt-5.2" while the Codex CLI on this machine actually serves
// gpt-5.6-sol and friends. So each provider is asked what it has:
//
//   Codex   ~/.codex/models_cache.json, which the CLI keeps current
//   Grok    `grok models`
//   Claude  the CLI's aliases, which resolve to whatever the subscription
//           currently provides — more durable than pinning an id
//   DeepSeek GET /v1/models with the key
//
// Every lookup falls back to a small built-in list, so the picker still works
// offline, before a key is entered, or if a CLI changes its output.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:convert';
import 'dart:io';

import '../engine/agent_cli.dart';
import 'models.dart';

/// One selectable model.
class ModelOption {
  /// The id passed to the CLI or API.
  final String id;

  /// What the picker shows.
  final String label;

  /// A one-line hint, when the source gives one.
  final String note;

  const ModelOption(this.id, this.label, {this.note = ''});

  @override
  bool operator ==(Object other) => other is ModelOption && other.id == id;

  @override
  int get hashCode => id.hashCode;
}

/// Where a model list came from, so the UI can be honest about it.
enum ModelSource {
  /// Read from the CLI or API for this provider.
  live,

  /// The built-in fallback, because the live lookup did not work.
  fallback,

  /// Nothing can be listed until a credential is supplied.
  needsKey,
}

/// A resolved model list plus how it was obtained.
class ModelList {
  final List<ModelOption> options;
  final ModelSource source;

  /// Why the live lookup failed, when it did.
  final String? problem;

  const ModelList(this.options, this.source, [this.problem]);
}

/// Loads and caches the models each provider offers.
class ModelCatalog {
  /// Cached per provider for the life of the process: a CLI call or an HTTP
  /// round trip is too slow to repeat every time a dropdown is built.
  static final Map<ProviderKind, ModelList> _cache = {};

  /// Overridable for tests: runs a command and returns its stdout.
  static Future<String> Function(String bin, List<String> args)? runner;

  /// Discard cached lists so the next lookup goes back to the source.
  static void invalidate() => _cache.clear();

  /// The models `kind` offers, from the machine where possible.
  static Future<ModelList> load(ProviderKind kind, {String apiKey = ''}) async {
    final cached = _cache[kind];
    if (cached != null) return cached;

    ModelList result;
    try {
      result = await _loadLive(kind, apiKey);
    } catch (e) {
      result = ModelList(fallbackFor(kind), ModelSource.fallback, e.toString());
    }
    if (result.options.isEmpty && result.source != ModelSource.needsKey) {
      result = ModelList(fallbackFor(kind), ModelSource.fallback, result.problem);
    }
    _cache[kind] = result;
    return result;
  }

  static Future<ModelList> _loadLive(ProviderKind kind, String apiKey) async {
    switch (kind) {
      case ProviderKind.codex:
        return _fromCodexCache();
      case ProviderKind.grok:
        return _fromGrokCli();
      case ProviderKind.anthropic:
        return ModelList(_claudeAliases, ModelSource.live);
      case ProviderKind.deepseek:
        return _fromDeepSeekApi(apiKey);
      case ProviderKind.scripted:
        return const ModelList([], ModelSource.live);
    }
  }

  // ---- sources ------------------------------------------------------------

  /// Claude Code takes aliases as well as ids. An alias follows whatever the
  /// subscription currently serves, so it does not need updating here when a
  /// new model ships.
  static const List<ModelOption> _claudeAliases = [
    ModelOption('opus', 'Opus', note: 'Best judgement'),
    ModelOption('sonnet', 'Sonnet', note: 'Fast and capable'),
    ModelOption('haiku', 'Haiku', note: 'Cheapest — good for inspectors'),
    ModelOption('', 'Subscription default', note: 'Whatever your plan selects'),
  ];

  /// Read the model list the Codex CLI keeps at ~/.codex/models_cache.json.
  static Future<ModelList> _fromCodexCache() async {
    final home = Platform.environment['HOME'] ?? '';
    if (home.isEmpty) {
      return ModelList(fallbackFor(ProviderKind.codex), ModelSource.fallback,
          'no HOME to find the Codex cache in');
    }
    final file = File('$home/.codex/models_cache.json');
    if (!file.existsSync()) {
      return ModelList(fallbackFor(ProviderKind.codex), ModelSource.fallback,
          'sign in with `codex` to populate its model list');
    }
    final root = jsonDecode(await file.readAsString());
    if (root is! Map || root['models'] is! List) {
      return ModelList(fallbackFor(ProviderKind.codex), ModelSource.fallback,
          'the Codex model cache is not in the expected shape');
    }
    final options = <ModelOption>[];
    for (final entry in root['models'] as List) {
      if (entry is! Map) continue;
      final slug = entry['slug'];
      if (slug is! String || slug.isEmpty) continue;
      // Hidden and internal entries are in the cache but are not choices.
      if (entry['visibility'] == 'hidden') continue;
      final display = entry['display_name'];
      options.add(ModelOption(
        slug,
        display is String && display.isNotEmpty ? display : slug,
        note: entry['description'] is String
            ? _firstSentence(entry['description'] as String)
            : '',
      ));
    }
    return ModelList(options, ModelSource.live);
  }

  /// Ask the Grok CLI what it can run. Its output is a plain list with the
  /// default marked by an asterisk.
  static Future<ModelList> _fromGrokCli() async {
    final status = findCli(CliKind.grok);
    if (!status.available) {
      return ModelList(fallbackFor(ProviderKind.grok), ModelSource.fallback,
          status.problem);
    }
    final out = await _run(status.path!, const ['models']);
    final options = <ModelOption>[];
    for (final line in const LineSplitter().convert(out)) {
      final trimmed = line.trim();
      // Entries look like "* grok-4.6 (default)" or "- grok-4.5".
      if (!trimmed.startsWith('*') && !trimmed.startsWith('-')) continue;
      final body = trimmed.substring(1).trim();
      if (body.isEmpty) continue;
      final isDefault = body.contains('(default)');
      final id = body.split(RegExp(r'\s+')).first;
      if (id.isEmpty) continue;
      options.add(ModelOption(id, id, note: isDefault ? 'Default' : ''));
    }
    return ModelList(options, ModelSource.live);
  }

  /// DeepSeek is a plain API, so its list comes from the models endpoint.
  static Future<ModelList> _fromDeepSeekApi(String apiKey) async {
    if (apiKey.trim().isEmpty) {
      // No key, no list. Showing invented ids here would be the hardcoding
      // this class exists to remove, and would look authoritative while being
      // a guess.
      return const ModelList([], ModelSource.needsKey,
          'Enter your API key to load DeepSeek\'s models.');
    }
    final client = HttpClient()..connectionTimeout = const Duration(seconds: 8);
    try {
      final request =
          await client.getUrl(Uri.parse('https://api.deepseek.com/v1/models'));
      request.headers.set('authorization', 'Bearer ' + apiKey.trim());
      final response = await request.close().timeout(const Duration(seconds: 12));
      final body = await response.transform(utf8.decoder).join();
      if (response.statusCode != 200) {
        return ModelList(fallbackFor(ProviderKind.deepseek),
            ModelSource.fallback, 'DeepSeek returned HTTP ${response.statusCode}');
      }
      final root = jsonDecode(body);
      final data = root is Map ? root['data'] : null;
      if (data is! List) {
        return ModelList(fallbackFor(ProviderKind.deepseek),
            ModelSource.fallback, 'unexpected response shape');
      }
      final options = <ModelOption>[];
      for (final entry in data) {
        if (entry is Map && entry['id'] is String) {
          options.add(ModelOption(entry['id'] as String, entry['id'] as String));
        }
      }
      return ModelList(options, ModelSource.live);
    } finally {
      client.close(force: true);
    }
  }

  static Future<String> _run(String bin, List<String> args) async {
    final override = runner;
    if (override != null) return override(bin, args);
    final result = await Process.run(bin, args,
        environment: const {'NO_COLOR': '1'}).timeout(const Duration(seconds: 20));
    return (result.stdout as String?) ?? '';
  }

  // ---- fallbacks ----------------------------------------------------------

  /// The built-in list used when a live lookup is not possible.
  static List<ModelOption> fallbackFor(ProviderKind kind) {
    switch (kind) {
      case ProviderKind.anthropic:
        return _claudeAliases;
      case ProviderKind.codex:
        return const [ModelOption('', 'Subscription default')];
      case ProviderKind.grok:
        return const [ModelOption('', 'Subscription default')];
      case ProviderKind.deepseek:
        // Deliberately empty: DeepSeek's list comes from its API, and a guess
        // presented as a list is worse than saying a key is needed.
        return const [];
      case ProviderKind.scripted:
        return const [];
    }
  }

  /// The id used when nothing has been chosen. Empty means "let the provider
  /// decide", which for a subscription-backed CLI is the right default.
  static String defaultFor(ProviderKind kind) {
    switch (kind) {
      case ProviderKind.deepseek:
        return 'deepseek-chat';
      default:
        return '';
    }
  }

  /// A label for an id, for list rows. Uses whatever has been cached; falls
  /// back to the raw id so a custom model still reads sensibly.
  static String labelFor(ProviderKind kind, String id) {
    if (id.isEmpty) {
      return kind == ProviderKind.scripted ? 'demo' : 'default model';
    }
    for (final option in (_cache[kind]?.options ?? fallbackFor(kind))) {
      if (option.id == id) return option.label;
    }
    return id;
  }

  static String _firstSentence(String text) {
    final stop = text.indexOf('.');
    final first = stop > 0 ? text.substring(0, stop) : text;
    return first.length > 60 ? first.substring(0, 60) + '…' : first;
  }
}
