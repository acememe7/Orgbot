// The models a bot can be pointed at, read from this machine rather than
// written down here.
//
// A hardcoded list goes stale and is wrong the day it ships — the previous one
// offered "gpt-5.2" while the Codex CLI on this machine actually serves
// gpt-5.6-sol and friends. Catalogs use each provider's supported source:
//
//   Codex   ~/.codex/models_cache.json, which the CLI keeps current
//   Grok    `grok models`
//   Claude  the CLI's aliases, which resolve to whatever the subscription
//           currently provides — more durable than pinning an id
//   DeepSeek / Ollama Cloud GET /v1/models with the key
//   OpenCode / Antigravity require a model id; their model/auth interfaces are
//           not queried here, so no provider default is inferred.
//
// If a live catalog is unavailable, the picker uses a provider-specific
// fallback or an empty list when guessing would be misleading.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:convert';
import 'dart:io';

import '../engine/agent_cli.dart';
import 'credentials.dart';
import 'models.dart';
import 'oauth/device_auth.dart';

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

  /// The stored subscription credential was rejected after renewal.
  needsSignIn,

  /// Requires an explicit model; the CLI was not queried for one.
  requiresExplicitModel,
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
  /// Cached per provider and CLI/API path for the life of the process.
  static final Map<(ProviderKind, bool), ModelList> _cache = {};

  /// Overridable for tests: runs a command and returns its stdout.
  static Future<String> Function(String bin, List<String> args)? runner;
  static Future<HttpResponse> Function(Uri url, Map<String, String> headers)?
  httpGet;
  static DeviceAuth? deviceAuth;

  /// Discard cached lists so the next lookup goes back to the source.
  static void invalidate() => _cache.clear();

  /// A paired sign-in, when one has been imported. Used on phones, which have
  /// no local agent CLIs.
  static Credentials? credentials;

  /// The models `kind` offers, from the machine or from a phone sign-in.
  static Future<ModelList> load(
    ProviderKind kind, {
    String apiKey = '',
    bool refresh = false,
    bool? canRunClis,
  }) async {
    final useClis = canRunClis ?? canRunAgentClis;
    // Desktop subscription catalogs come from local CLIs. Do not touch the
    // phone OAuth store on that path; API-key providers still need it.
    if (!useClis || kind.needsApiKey) await credentials?.load();
    if (apiKey.trim().isEmpty && kind.needsApiKey) {
      apiKey = credentials?.forProvider(kind)?.token ?? '';
    }
    final cacheKey = (kind, useClis);
    final cached = _cache[cacheKey];
    final paired = useClis ? null : credentials?.forProvider(kind);
    if (!refresh &&
        !kind.needsApiKey &&
        cached?.source == ModelSource.live &&
        paired?.needsRenewal != true) {
      return cached!;
    }

    // On a phone, a paired sign-in carries the desktop CLI's model list.
    if (!refresh &&
        !kind.needsApiKey &&
        paired != null &&
        !paired.needsRenewal &&
        paired.models.isNotEmpty) {
      final list = ModelList(paired.models, ModelSource.live);
      _cache[cacheKey] = list;
      return list;
    }

    ModelList result;
    try {
      result = await _loadLive(kind, apiKey, canRunClis: useClis);
    } catch (e) {
      result = ModelList(
        fallbackFor(kind, canRunClis: useClis),
        ModelSource.fallback,
        e.toString(),
      );
    }
    if (result.options.isEmpty && result.source == ModelSource.live) {
      result = ModelList(
        fallbackFor(kind, canRunClis: useClis),
        ModelSource.fallback,
        result.problem,
      );
    }
    _cache[cacheKey] = result;
    return result;
  }

  static Future<ModelList> _loadLive(
    ProviderKind kind,
    String apiKey, {
    required bool canRunClis,
  }) async {
    final credential = canRunClis ? null : credentials?.forProvider(kind);
    var token = canRunClis
        ? null
        : await credentials?.freshToken(kind, auth: deviceAuth);
    if (credential != null && kind.signsInOnDevice) {
      for (var attempt = 0; attempt < 2; attempt++) {
        if (token == null) break;
        try {
          return kind == ProviderKind.grok
              ? await _fromGrokApi(token)
              : await _fromCodexApi(token);
        } on _CredentialRejected {
          if (attempt == 1) break;
          token = await credentials?.freshToken(
            kind,
            auth: deviceAuth,
            force: true,
          );
        }
      }
      return const ModelList(
        [],
        ModelSource.needsSignIn,
        'The provider rejected this sign-in after renewal. Sign in again from Accounts.',
      );
    }
    switch (kind) {
      case ProviderKind.codex:
        return token == null
            ? canRunClis
                  ? _fromCodexCache(canRunClis: true)
                  : ModelList(
                      fallbackFor(ProviderKind.codex, canRunClis: false),
                      ModelSource.fallback,
                      'Sign in on this device to load Codex models.',
                    )
            : _fromCodexApi(token);
      case ProviderKind.grok:
        return token == null
            ? canRunClis
                  ? _fromGrokCli(canRunClis: true)
                  : ModelList(
                      fallbackFor(ProviderKind.grok, canRunClis: false),
                      ModelSource.fallback,
                      'Sign in on this device to load Grok models.',
                    )
            : _fromGrokApi(token);
      case ProviderKind.anthropic:
        return ModelList(claudeModels(viaCli: canRunClis), ModelSource.live);
      case ProviderKind.opencode:
      case ProviderKind.antigravity:
        return canRunClis && Platform.isLinux
            ? ModelList(const [], ModelSource.requiresExplicitModel)
            : ModelList(
                const [],
                ModelSource.fallback,
                '${kind.label} requires the protected Linux runtime.',
              );
      case ProviderKind.deepseek:
      case ProviderKind.ollama:
        return _fromApi(kind, apiKey);
      case ProviderKind.scripted:
        return const ModelList([], ModelSource.live);
    }
  }

  // ---- sources ------------------------------------------------------------

  /// Claude models, named the way the caller will actually reach them.
  ///
  /// Claude Code accepts `opus` and `sonnet` and resolves them against
  /// whatever the plan currently serves, which is why they were listed here.
  /// But a phone has no CLI to do that resolving: it reaches Claude through
  /// the Messages API, which takes ids only and rejects an alias outright. A
  /// list that only works where a CLI happens to be installed is not a list.
  ///
  /// `viaCli` is a parameter rather than a read of the host so this can be
  /// checked for the platform it is wrong on, from the platform it is right
  /// on.
  static List<ModelOption> claudeModels({required bool viaCli}) => viaCli
      ? const [
          ModelOption('opus', 'Opus', note: 'Best judgement'),
          ModelOption('sonnet', 'Sonnet', note: 'Fast and capable'),
          ModelOption('haiku', 'Haiku', note: 'Cheapest — good for inspectors'),
          ModelOption(
            '',
            'Subscription default',
            note: 'Whatever your plan selects',
          ),
        ]
      : const [
          ModelOption('claude-opus-5', 'Opus 5', note: 'Best judgement'),
          ModelOption('claude-sonnet-5', 'Sonnet 5', note: 'Fast and capable'),
          ModelOption(
            'claude-haiku-4-5-20251001',
            'Haiku 4.5',
            note: 'Cheapest — good for inspectors',
          ),
        ];

  /// Read the model list the Codex CLI keeps at ~/.codex/models_cache.json.
  static Future<ModelList> _fromCodexCache({required bool canRunClis}) async {
    final home = Platform.environment['HOME'] ?? '';
    if (home.isEmpty) {
      return ModelList(
        fallbackFor(ProviderKind.codex, canRunClis: canRunClis),
        ModelSource.fallback,
        'no HOME to find the Codex cache in',
      );
    }
    final file = File('$home/.codex/models_cache.json');
    if (!file.existsSync()) {
      return ModelList(
        fallbackFor(ProviderKind.codex, canRunClis: canRunClis),
        ModelSource.fallback,
        'sign in with `codex` to populate its model list',
      );
    }
    final root = jsonDecode(await file.readAsString());
    if (root is! Map || root['models'] is! List) {
      return ModelList(
        fallbackFor(ProviderKind.codex, canRunClis: canRunClis),
        ModelSource.fallback,
        'the Codex model cache is not in the expected shape',
      );
    }
    final options = <ModelOption>[];
    for (final entry in root['models'] as List) {
      if (entry is! Map) continue;
      final slug = entry['slug'];
      if (slug is! String || slug.isEmpty) continue;
      // Hidden and internal entries are in the cache but are not choices.
      if (_isHidden(entry['visibility'])) continue;
      final display = entry['display_name'];
      options.add(
        ModelOption(
          slug,
          display is String && display.isNotEmpty ? display : slug,
          note: entry['description'] is String
              ? _firstSentence(entry['description'] as String)
              : '',
        ),
      );
    }
    return ModelList(options, ModelSource.live);
  }

  /// Ask the Grok CLI what it can run. Its output is a plain list with the
  /// default marked by an asterisk.
  static Future<ModelList> _fromGrokCli({required bool canRunClis}) async {
    final status = runner == null
        ? findCli(CliKind.grok)
        : const CliStatus('grok', null);
    if (!status.available) {
      return ModelList(
        fallbackFor(ProviderKind.grok, canRunClis: canRunClis),
        ModelSource.fallback,
        status.problem,
      );
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

  /// Whether a model entry is one the provider does not mean to offer.
  ///
  /// The Codex catalogue marks these `hide`, not `hidden` — a filter written
  /// against the wrong spelling matched nothing and put internal entries like
  /// `gpt-reserve` in the picker, where choosing one gets a bot rejected on
  /// every turn.
  static bool _isHidden(Object? visibility) =>
      visibility == 'hide' || visibility == 'hidden';

  /// The Codex client version the models endpoint is asked for. It is a
  /// required query parameter, and the backend tailors the list to it.
  static const String _codexClientVersion = '0.153.2';

  /// Ask the ChatGPT backend what a signed-in subscription may run.
  static Future<ModelList> _fromCodexApi(String token) async {
    final body = await _get(
      Uri.parse(
        'https://chatgpt.com/backend-api/codex/models'
        '?client_version=$_codexClientVersion',
      ),
      {
        'authorization': 'Bearer ' + token,
        'originator': 'codex_cli_rs',
        'user-agent': 'codex_cli_rs/$_codexClientVersion (OrgBot)',
        'accept': 'application/json',
      },
      ProviderKind.codex,
    );
    if (body is ModelList) return body;
    final root = jsonDecode(body as String);
    final models = root is Map ? root['models'] : null;
    if (models is! List) {
      return ModelList(
        fallbackFor(ProviderKind.codex, canRunClis: false),
        ModelSource.fallback,
        'unexpected response shape',
      );
    }
    final options = <ModelOption>[];
    for (final entry in models) {
      if (entry is! Map) continue;
      final slug = entry['slug'];
      if (slug is! String || slug.isEmpty) continue;
      if (_isHidden(entry['visibility'])) continue;
      final display = entry['display_name'];
      options.add(
        ModelOption(
          slug,
          display is String && display.isNotEmpty ? display : slug,
          note: entry['description'] is String
              ? _firstSentence(entry['description'] as String)
              : '',
        ),
      );
    }
    return ModelList(options, ModelSource.live);
  }

  /// Subscription models come from the same catalog as the Grok CLI.
  static Future<ModelList> _fromGrokApi(String token) async {
    final subscription =
        credentials?.forProvider(ProviderKind.grok)?.kind !=
        CredentialKind.apiKey;
    final body = await _get(
      Uri.parse(
        subscription
            ? 'https://cli-chat-proxy.grok.com/v1/models'
            : 'https://api.x.ai/v1/models',
      ),
      {
        'authorization': 'Bearer ' + token,
        'accept': 'application/json',
        if (subscription) 'X-XAI-Token-Auth': 'xai-grok-cli',
        if (subscription) 'x-authenticateresponse': 'authenticate-response',
      },
      ProviderKind.grok,
    );
    if (body is ModelList) return body;
    final root = jsonDecode(body as String);
    final data = root is Map ? root['data'] : null;
    if (data is! List) {
      return ModelList(
        fallbackFor(ProviderKind.grok, canRunClis: false),
        ModelSource.fallback,
        'unexpected response shape',
      );
    }
    final options = <ModelOption>[];
    for (final entry in data) {
      if (entry is! Map) continue;
      final id = entry['id'];
      if (id is! String || id.isEmpty) continue;
      options.add(ModelOption(id, entry['name'] as String? ?? id));
    }
    return ModelList(options, ModelSource.live);
  }

  /// GET a URL, returning the body on success or a ready-made fallback list
  /// describing what went wrong. Shared by the two subscription endpoints.
  static Future<Object> _get(
    Uri url,
    Map<String, String> headers,
    ProviderKind kind,
  ) async {
    final response = await (httpGet ?? _httpGet)(url, headers);
    if (response.status == 401 ||
        (response.status == 403 &&
            response.body.contains('unauthenticated:bad-credentials'))) {
      throw const _CredentialRejected();
    }
    if (response.status != 200) {
      return ModelList(
        fallbackFor(kind, canRunClis: false),
        ModelSource.fallback,
        response.status == 403
            ? 'The account could not access this model catalog (HTTP 403). Its sign-in is still saved.'
            : 'The model catalog returned HTTP ${response.status}. Retry when the provider is available.',
      );
    }
    return response.body;
  }

  static Future<HttpResponse> _httpGet(
    Uri url,
    Map<String, String> headers,
  ) async {
    final client = HttpClient()..connectionTimeout = const Duration(seconds: 8);
    try {
      final request = await client.getUrl(url);
      headers.forEach(request.headers.set);
      final response = await request.close().timeout(
        const Duration(seconds: 12),
      );
      final body = await response
          .transform(utf8.decoder)
          .join()
          .timeout(const Duration(seconds: 12));
      return HttpResponse(response.statusCode, body);
    } finally {
      client.close(force: true);
    }
  }

  /// Saved API accounts share the OpenAI-compatible models endpoint.
  static Future<ModelList> _fromApi(ProviderKind kind, String apiKey) async {
    if (apiKey.trim().isEmpty) {
      // No key, no list. Showing invented ids here would be the hardcoding
      // this class exists to remove, and would look authoritative while being
      // a guess.
      return ModelList(
        [],
        ModelSource.needsKey,
        'Add your ${kind.label} API key in Accounts to load models.',
      );
    }
    try {
      final body = await _get(Uri.parse('${kind.apiBaseUrl}/v1/models'), {
        'authorization': 'Bearer ${apiKey.trim()}',
        'accept': 'application/json',
      }, kind);
      if (body is ModelList) return body;
      final root = jsonDecode(body as String);
      final data = root is Map ? root['data'] : null;
      if (data is! List) {
        return ModelList(
          fallbackFor(kind),
          ModelSource.fallback,
          'unexpected response shape',
        );
      }
      final options = <ModelOption>[];
      for (final entry in data) {
        if (entry is Map &&
            entry['id'] is String &&
            (entry['id'] as String).trim().isNotEmpty) {
          options.add(
            ModelOption(entry['id'] as String, entry['id'] as String),
          );
        }
      }
      return ModelList(
        options.toSet().toList(),
        ModelSource.live,
        options.isEmpty ? 'No models were returned by ${kind.label}.' : null,
      );
    } on _CredentialRejected {
      return ModelList(
        const [],
        ModelSource.needsKey,
        '${kind.label} rejected this API key. Replace it in Accounts.',
      );
    }
  }

  static Future<String> _run(String bin, List<String> args) async {
    final override = runner;
    if (override != null) return override(bin, args);
    final result = await Process.run(
      bin,
      args,
      environment: const {'NO_COLOR': '1'},
    ).timeout(const Duration(seconds: 20));
    return (result.stdout as String?) ?? '';
  }

  // ---- fallbacks ----------------------------------------------------------

  /// The built-in list used when a live lookup is not possible.
  static List<ModelOption> fallbackFor(ProviderKind kind, {bool? canRunClis}) {
    final cli = kind.cli;
    final useClis =
        (canRunClis ?? canRunAgentClis) &&
        (!(cli?.requiresProtectedRuntime ?? false) || Platform.isLinux);
    switch (kind) {
      case ProviderKind.anthropic:
        return claudeModels(viaCli: useClis);
      case ProviderKind.codex:
      case ProviderKind.grok:
        if (!useClis) return const [];
        return const [ModelOption('', 'Subscription default')];
      case ProviderKind.deepseek:
      case ProviderKind.ollama:
        // Deliberately empty: the list comes from its API, and a guess
        // presented as a list is worse than saying a key is needed.
        return const [];
      case ProviderKind.opencode:
      case ProviderKind.antigravity:
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
    for (final option
        in (_cache[(kind, canRunAgentClis)]?.options ?? fallbackFor(kind))) {
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

class _CredentialRejected implements Exception {
  const _CredentialRejected();
}
