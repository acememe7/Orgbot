/// Read-only discovery of MCP servers and skills already configured on this PC.
///
/// Status objects intentionally contain no environment values or HTTP headers.
/// Runnable configuration is added to a room only through [appendRoomConfig],
/// which moves credential values into the room launch environment and writes
/// `${NAME}` references to the private, ephemeral room file.
library;

import 'dart:convert';
import 'dart:io';

/// A configured integration source on this machine.
enum DesktopIntegrationSource { codex, claude, grok }

/// How an integration is configured locally.
enum DesktopIntegrationTransport { stdio, http, unsupported }

/// Safe, displayable connection status. `configured` does not claim the server
/// has connected or authenticated; the runtime reports that separately.
enum DesktopIntegrationState { configured, unsupported, unavailable }

/// Sanitized status for a configured MCP server.
class DesktopMcpStatus {
  final String name;
  final DesktopIntegrationSource source;
  final DesktopIntegrationTransport transport;
  final DesktopIntegrationState state;
  final String detail;

  const DesktopMcpStatus({
    required this.name,
    required this.source,
    required this.transport,
    required this.state,
    required this.detail,
  });
}

/// A discovered skill folder. The skill file itself is loaded by the engine,
/// not read or copied by desktop discovery.
class DesktopSkillStatus {
  final String name;
  final String path;
  final DesktopIntegrationSource source;

  const DesktopSkillStatus({
    required this.name,
    required this.path,
    required this.source,
  });
}

class _McpSpec {
  final String name;
  final DesktopIntegrationSource source;
  final DesktopIntegrationTransport transport;
  final String? command;
  final List<String> args;
  final Map<String, String> env;
  final String? url;
  final Map<String, String> headers;
  final String? unsupportedReason;

  const _McpSpec({
    required this.name,
    required this.source,
    required this.transport,
    this.command,
    this.args = const [],
    this.env = const {},
    this.url,
    this.headers = const {},
    this.unsupportedReason,
  });
}

class _CodexPlugin {
  final String name;
  final bool hasMcpServers;

  const _CodexPlugin(this.name, this.hasMcpServers);
}

/// Snapshot of local integrations. Public statuses never include credentials.
class DesktopIntegrationSnapshot {
  final List<DesktopMcpStatus> mcpServers;
  final List<DesktopSkillStatus> skills;
  final List<_McpSpec> _runnable;

  const DesktopIntegrationSnapshot._(
    this.mcpServers,
    this.skills,
    this._runnable,
  );

  /// Add configured MCP servers and skills to an existing generated room file.
  /// Credential values are copied to [environment] under generated variable
  /// names; the TOML receives only `${VARIABLE}` placeholders.
  void appendRoomConfig(
    StringBuffer output, {
    required Map<String, String> environment,
  }) {
    for (var index = 0; index < _runnable.length; index++) {
      final server = _runnable[index];
      output
        ..writeln('[[mcp]]')
        ..writeln(
          'name = ${_tomlString('${server.source.name}__${server.name}')}',
        )
        ..writeln('transport = ${_tomlString(server.transport.name)}');
      if (server.transport == DesktopIntegrationTransport.stdio) {
        output
          ..writeln(
            'command = ${_environmentReference(environment, index, 'COMMAND', server.command!)}',
          )
          ..writeln(
            'args = ${_environmentArray(environment, index, server.args)}',
          );
        if (server.env.isNotEmpty) {
          output.writeln('[mcp.env]');
          for (final entry in server.env.entries) {
            output.writeln(
              '${_tomlKey(entry.key)} = ${_environmentReference(environment, index, 'ENV_${entry.key}', entry.value)}',
            );
          }
        }
      } else {
        output.writeln(
          'url = ${_environmentReference(environment, index, 'URL', server.url!)}',
        );
        if (server.headers.isNotEmpty) {
          output.writeln('[mcp.headers]');
          for (final entry in server.headers.entries) {
            output.writeln(
              '${_tomlKey(entry.key)} = ${_environmentReference(environment, index, 'HEADER_${entry.key}', entry.value)}',
            );
          }
        }
      }
      output.writeln();
    }
    for (final skill in skills) {
      output
        ..writeln('[[skills]]')
        ..writeln('path = ${_tomlString(skill.path)}')
        ..writeln();
    }
  }
}

/// Discovers active direct MCP entries and installed skill folders without
/// probing servers, invoking CLIs, or reading plugin/credential logs.
class DesktopIntegrations {
  const DesktopIntegrations._();

  static Future<DesktopIntegrationSnapshot> discover({
    String? homeDirectory,
  }) async {
    final home = homeDirectory ?? Platform.environment['HOME'] ?? '';
    if (home.isEmpty) {
      return const DesktopIntegrationSnapshot._([], [], []);
    }
    final specs = <_McpSpec>[];
    await _readCodex(home, specs);
    await _readClaude(home, specs);
    await _readGrok(home, specs);

    final statuses = <DesktopMcpStatus>[];
    final runnable = <_McpSpec>[];
    for (final spec in specs) {
      final unsupported = spec.unsupportedReason;
      final state = unsupported != null
          ? DesktopIntegrationState.unsupported
          : DesktopIntegrationState.configured;
      statuses.add(
        DesktopMcpStatus(
          name: spec.name,
          source: spec.source,
          transport: spec.transport,
          state: state,
          detail:
              unsupported ??
              'Configured locally; connection has not been tested.',
        ),
      );
      if (state == DesktopIntegrationState.configured) runnable.add(spec);
    }
    final skills = await _discoverSkills(home);
    return DesktopIntegrationSnapshot._(
      List.unmodifiable(statuses),
      List.unmodifiable(skills),
      List.unmodifiable(runnable),
    );
  }

  static Future<void> _readCodex(String home, List<_McpSpec> output) async {
    final file = File('$home/.codex/config.toml');
    try {
      final text = await file.readAsString();
      for (final entry in _parseTomlServers(text)) {
        output.add(_fromToml(DesktopIntegrationSource.codex, entry));
      }
      for (final plugin in _parseTomlPlugins(text)) {
        if (plugin.hasMcpServers) {
          output.add(
            _unsupported(
              DesktopIntegrationSource.codex,
              plugin.name,
              'Plugin-managed MCP tools cannot be imported into API-backed bots.',
            ),
          );
        }
      }
    } on FileSystemException {
      // An absent or unreadable optional config does not block room startup.
    }
  }

  static Future<void> _readGrok(String home, List<_McpSpec> output) async {
    final file = File('$home/.grok/config.toml');
    try {
      final text = await file.readAsString();
      for (final entry in _parseTomlServers(text)) {
        output.add(_fromToml(DesktopIntegrationSource.grok, entry));
      }
    } on FileSystemException {
      // Optional local configuration.
    }
  }

  static Future<void> _readClaude(String home, List<_McpSpec> output) async {
    for (final path in ['$home/.claude.json', '$home/.claude/settings.json']) {
      try {
        final decoded = jsonDecode(await File(path).readAsString());
        if (decoded is! Map) continue;
        final entries = decoded['mcpServers'];
        if (entries is! Map) continue;
        for (final item in entries.entries) {
          if (item.key is! String || item.value is! Map) continue;
          final entry = Map<String, Object?>.from(item.value as Map);
          if (entry['disabled'] == true || entry['enabled'] == false) continue;
          output.add(
            _fromJson(
              DesktopIntegrationSource.claude,
              item.key as String,
              entry,
            ),
          );
        }
      } on FileSystemException {
        // Optional local configuration.
      } on FormatException {
        // Invalid config is ignored; the native CLI keeps its own behavior.
      }
    }
  }

  static Future<List<DesktopSkillStatus>> _discoverSkills(String home) async {
    final roots = <(String, DesktopIntegrationSource)>[
      ('$home/.agents/skills', DesktopIntegrationSource.codex),
      ('$home/.codex/skills', DesktopIntegrationSource.codex),
      ('$home/.claude/skills', DesktopIntegrationSource.claude),
      ('$home/.grok/skills', DesktopIntegrationSource.grok),
    ];
    final codexConfig = File('$home/.codex/config.toml');
    try {
      final text = await codexConfig.readAsString();
      for (final plugin in _parseTomlPlugins(text)) {
        final identity = plugin.name.split('@');
        if (identity.length != 2) continue;
        final pluginRoot = Directory(
          '$home/.codex/plugins/cache/${identity[1]}/${identity[0]}',
        );
        try {
          await for (final version in pluginRoot.list(followLinks: false)) {
            if (version is Directory) {
              roots.add((
                '${version.path}/skills',
                DesktopIntegrationSource.codex,
              ));
            }
          }
        } on FileSystemException {
          // An enabled plugin may not be installed on this machine.
        }
      }
    } on FileSystemException {
      // The direct roots remain useful when there is no Codex config.
    }
    // Claude plugin skill folders are included only for plugins enabled in the
    // user's settings and listed as installed by Claude Code.
    try {
      final settings = jsonDecode(
        await File('$home/.claude/settings.json').readAsString(),
      );
      final installed = jsonDecode(
        await File(
          '$home/.claude/plugins/installed_plugins.json',
        ).readAsString(),
      );
      final enabled = settings is Map ? settings['enabledPlugins'] : null;
      final pluginMap = installed is Map ? installed['plugins'] : null;
      if (enabled is Map && pluginMap is Map) {
        for (final entry in enabled.entries) {
          if (entry.value != true) continue;
          final installs = pluginMap[entry.key];
          if (installs is! List) continue;
          for (final install in installs) {
            if (install is Map && install['installPath'] is String) {
              roots.add((
                '${install['installPath']}/skills',
                DesktopIntegrationSource.claude,
              ));
            }
          }
        }
      }
    } on FileSystemException {
      // Plugin manifests are optional.
    } on FormatException {
      // Ignore malformed plugin metadata.
    }

    final found = <String, DesktopSkillStatus>{};
    for (final (rootPath, source) in roots) {
      final root = Directory(rootPath);
      try {
        if (!await root.exists()) continue;
        if (await File('$rootPath/SKILL.md').exists()) {
          final name = root.uri.pathSegments
              .where((part) => part.isNotEmpty)
              .last;
          found.putIfAbsent(
            rootPath,
            () =>
                DesktopSkillStatus(name: name, path: rootPath, source: source),
          );
          continue;
        }
        await for (final entry in root.list(followLinks: false)) {
          if (entry is! Directory ||
              !await File('${entry.path}/SKILL.md').exists()) {
            continue;
          }
          final name = entry.uri.pathSegments
              .where((part) => part.isNotEmpty)
              .last;
          found.putIfAbsent(
            entry.path,
            () => DesktopSkillStatus(
              name: name,
              path: entry.path,
              source: source,
            ),
          );
        }
      } on FileSystemException {
        // A missing or inaccessible skills folder is reported by omission.
      }
    }
    final result = found.values.toList()
      ..sort((a, b) => a.name.compareTo(b.name));
    return result;
  }
}

_McpSpec _fromJson(
  DesktopIntegrationSource source,
  String name,
  Map<String, Object?> entry,
) {
  final type = entry['type'] as String?;
  final filterReason = _toolFilterReason(entry);
  if (filterReason != null) return _unsupported(source, name, filterReason);
  if (type == 'sse') {
    return _unsupported(
      source,
      name,
      'Legacy server-sent events transport is not supported for API-backed bots.',
    );
  }
  if (entry['url'] is String || type == 'http') {
    final url = entry['url'] as String?;
    if (url == null) {
      return _unsupported(
        source,
        name,
        'The configured HTTP endpoint is incomplete.',
      );
    }
    if (entry['oauth'] != null || entry['auth'] == 'oauth') {
      return _unsupported(
        source,
        name,
        'OAuth-authenticated MCP is not supported for API-backed bots.',
      );
    }
    final resolvedUrl = _expandConfigString(url);
    if (resolvedUrl == null) {
      return _unsupported(
        source,
        name,
        'The HTTP endpoint needs an unavailable local environment value.',
      );
    }
    final headers = _expandSecretMap(_stringMap(entry['headers']));
    if (headers == null) {
      return _unsupported(
        source,
        name,
        'A configured HTTP header needs an unavailable local environment value.',
      );
    }
    return _McpSpec(
      name: name,
      source: source,
      transport: DesktopIntegrationTransport.http,
      url: resolvedUrl,
      headers: headers,
    );
  }
  final command = entry['command'] as String?;
  if (command == null) {
    return _unsupported(
      source,
      name,
      'The configured server transport is unsupported.',
    );
  }
  final resolvedCommand = _expandConfigString(command);
  final args = _expandConfigList(_stringList(entry['args']));
  if (resolvedCommand == null || args == null) {
    return _unsupported(
      source,
      name,
      'The configured process command needs an unavailable local environment value.',
    );
  }
  final env = _expandSecretMap(_stringMap(entry['env']));
  if (env == null) {
    return _unsupported(
      source,
      name,
      'A configured process environment value is unavailable locally.',
    );
  }
  return _McpSpec(
    name: name,
    source: source,
    transport: DesktopIntegrationTransport.stdio,
    command: resolvedCommand,
    args: args,
    env: env,
  );
}

_McpSpec _fromToml(
  DesktopIntegrationSource source,
  Map<String, Object?> entry,
) {
  final name = entry['_name'] as String;
  final filterReason = _toolFilterReason(entry);
  if (filterReason != null) return _unsupported(source, name, filterReason);
  if (entry['type'] == 'sse' || entry['transport'] == 'sse') {
    return _unsupported(
      source,
      name,
      'Legacy server-sent events transport is not supported for API-backed bots.',
    );
  }
  final url = entry['url'] as String?;
  if (url != null) {
    if (entry['oauth'] != null) {
      return _unsupported(
        source,
        name,
        'OAuth-authenticated MCP is not supported for API-backed bots.',
      );
    }
    final resolvedUrl = _expandConfigString(url);
    if (resolvedUrl == null) {
      return _unsupported(
        source,
        name,
        'The HTTP endpoint needs an unavailable local environment value.',
      );
    }
    final headers = _stringMap(entry['http_headers']);
    final envHeaders = _stringMap(entry['env_http_headers']);
    final bearerName = entry['bearer_token_env_var'] as String?;
    if (bearerName != null) {
      final token = Platform.environment[bearerName];
      if (token == null) {
        return _unsupported(
          source,
          name,
          'A configured HTTP credential needs an unavailable local environment value.',
        );
      }
      headers['Authorization'] = 'Bearer $token';
    }
    for (final item in envHeaders.entries) {
      final value = Platform.environment[item.value];
      if (value == null) {
        return _unsupported(
          source,
          name,
          'A configured HTTP header needs an unavailable local environment value.',
        );
      }
      headers[item.key] = value;
    }
    final resolvedHeaders = _expandSecretMap(headers);
    if (resolvedHeaders == null) {
      return _unsupported(
        source,
        name,
        'A configured HTTP header needs an unavailable local environment value.',
      );
    }
    return _McpSpec(
      name: name,
      source: source,
      transport: DesktopIntegrationTransport.http,
      url: resolvedUrl,
      headers: resolvedHeaders,
    );
  }
  final command = entry['command'] as String?;
  if (command == null) {
    return _unsupported(
      source,
      name,
      'The configured server transport is unsupported.',
    );
  }
  final resolvedCommand = _expandConfigString(command);
  final args = _expandConfigList(_stringList(entry['args']));
  if (resolvedCommand == null || args == null) {
    return _unsupported(
      source,
      name,
      'The configured process command needs an unavailable local environment value.',
    );
  }
  final env = _expandSecretMap(_stringMap(entry['env']));
  if (env == null) {
    return _unsupported(
      source,
      name,
      'A configured process environment value is unavailable locally.',
    );
  }
  return _McpSpec(
    name: name,
    source: source,
    transport: DesktopIntegrationTransport.stdio,
    command: resolvedCommand,
    args: args,
    env: env,
  );
}

_McpSpec _unsupported(
  DesktopIntegrationSource source,
  String name,
  String reason,
) => _McpSpec(
  name: name,
  source: source,
  transport: DesktopIntegrationTransport.unsupported,
  unsupportedReason: reason,
);

List<Map<String, Object?>> _parseTomlServers(String text) {
  final result = <String, Map<String, Object?>>{};
  final disabled = <String>{};
  String? active;
  String? subsection;
  for (final raw in const LineSplitter().convert(text)) {
    final line = _stripTomlComment(raw).trim();
    if (line.isEmpty || line.startsWith('#')) continue;
    final table = RegExp(r'^\[\[?([^\]]+)\]\]?$').firstMatch(line);
    if (table != null) {
      final name = RegExp(
        r'^mcp_servers\.(?:"([^"]+)"|([A-Za-z0-9_-]+))(?:\.(.+))?$',
      ).firstMatch(table.group(1)!.trim());
      if (name == null) {
        active = null;
        subsection = null;
      } else {
        active = name.group(1) ?? name.group(2)!;
        final tail = name.group(3);
        subsection = switch (tail) {
          null => null,
          'env' || 'http_headers' || 'env_http_headers' => tail,
          _ => null,
        };
        if (disabled.contains(active)) {
          active = null;
          subsection = null;
          continue;
        }
        result.putIfAbsent(active, () => <String, Object?>{'_name': active});
        if (tail == 'tools' || tail?.startsWith('tools.') == true) {
          // Codex tool-level approvals and allow/deny settings cannot be
          // represented by the API room's coarser MCP grant. Fail closed.
          result[active]!['_unsupported_tool_subtable'] = true;
          active = null;
          subsection = null;
          continue;
        }
        if (subsection != null) {
          result[active]!.putIfAbsent(subsection, () => <String, Object?>{});
        } else if (tail != null) {
          // Do not import unknown MCP subtables with semantics we do not
          // understand. In particular, never silently drop restrictions.
          result[active]!['_unsupported_mcp_subtable'] = true;
          active = null;
          continue;
        }
      }
      continue;
    }
    if (active == null) continue;
    final assignment = RegExp(
      r'^([A-Za-z0-9_-]+)\s*=\s*(.*?)\s*$',
    ).firstMatch(line);
    if (assignment == null) continue;
    final key = assignment.group(1)!;
    final value = _parseTomlValue(assignment.group(2)!);
    if (subsection == null &&
        ((key == 'enabled' && value == false) ||
            (key == 'disabled' && value == true))) {
      disabled.add(active);
      result.remove(active);
      active = null;
      subsection = null;
      continue;
    }
    if (subsection != null) {
      final nested = result[active]![subsection];
      if (nested is Map<String, Object?>) nested[key] = value;
    } else {
      result[active]![key] = value;
    }
  }
  return result.values.toList();
}

List<_CodexPlugin> _parseTomlPlugins(String text) {
  final plugins = <String, ({bool enabled, bool hasMcp})>{};
  String? active;
  for (final raw in const LineSplitter().convert(text)) {
    final line = _stripTomlComment(raw).trim();
    if (line.isEmpty) continue;
    final table = RegExp(r'^\[([^\]]+)\]$').firstMatch(line);
    if (table != null) {
      final plugin = RegExp(
        r'^plugins\.(?:"([^"]+)"|([A-Za-z0-9_-]+))$',
      ).firstMatch(table.group(1)!.trim());
      active = plugin == null ? null : (plugin.group(1) ?? plugin.group(2)!);
      if (active != null) {
        plugins.putIfAbsent(active, () => (enabled: false, hasMcp: false));
      }
      continue;
    }
    if (active == null) continue;
    final assignment = RegExp(
      r'^([A-Za-z0-9_-]+)\s*=\s*(.*?)\s*$',
    ).firstMatch(line);
    if (assignment == null) continue;
    final current = plugins[active]!;
    if (assignment.group(1) == 'enabled') {
      plugins[active] = (
        enabled: _parseTomlValue(assignment.group(2)!) == true,
        hasMcp: current.hasMcp,
      );
    } else if (assignment.group(1) == 'mcp_servers') {
      plugins[active] = (enabled: current.enabled, hasMcp: true);
    }
  }
  return plugins.entries
      .where((entry) => entry.value.enabled)
      .map((entry) => _CodexPlugin(entry.key, entry.value.hasMcp))
      .toList();
}

Object? _parseTomlValue(String raw) {
  final value = raw.trim();
  if (value == 'true') return true;
  if (value == 'false') return false;
  if (value.startsWith('"') && value.endsWith('"')) {
    try {
      return jsonDecode(value);
    } on FormatException {
      return value.substring(1, value.length - 1);
    }
  }
  if (value.startsWith('[') && value.endsWith(']')) {
    final body = value.substring(1, value.length - 1).trim();
    if (body.isEmpty) return <String>[];
    return RegExp(r'"((?:\\.|[^"])*)"').allMatches(body).map((match) {
      try {
        return jsonDecode('"${match.group(1)}"') as String;
      } on FormatException {
        return match.group(1)!;
      }
    }).toList();
  }
  final inline = RegExp(r'^\{\s*(.*?)\s*\}$').firstMatch(value);
  if (inline != null) {
    final map = <String, Object?>{};
    for (final pair in RegExp(
      r'([A-Za-z0-9_-]+)\s*=\s*("(?:\\.|[^"])*")',
    ).allMatches(inline.group(1)!)) {
      map[pair.group(1)!] = _parseTomlValue(pair.group(2)!);
    }
    return map;
  }
  return value;
}

Map<String, String> _stringMap(Object? raw) {
  if (raw is! Map) return <String, String>{};
  return Map<String, String>.fromEntries(
    raw.entries
        .where((entry) => entry.key is String && entry.value is String)
        .map((entry) => MapEntry(entry.key as String, entry.value as String)),
  );
}

Map<String, String>? _expandSecretMap(Map<String, String> values) {
  final expanded = <String, String>{};
  final references = RegExp(r'\$\{([A-Za-z_][A-Za-z0-9_]*)\}');
  for (final entry in values.entries) {
    var value = entry.value;
    for (final match in references.allMatches(entry.value).toList().reversed) {
      final replacement = Platform.environment[match.group(1)];
      if (replacement == null) return null;
      value = value.replaceRange(match.start, match.end, replacement);
    }
    expanded[entry.key] = value;
  }
  return expanded;
}

List<String>? _expandConfigList(List<String> values) {
  final result = <String>[];
  for (final value in values) {
    final expanded = _expandConfigString(value);
    if (expanded == null) return null;
    result.add(expanded);
  }
  return result;
}

String? _expandConfigString(String value) {
  var expanded = value;
  final references = RegExp(r'\$\{([A-Za-z_][A-Za-z0-9_]*)\}');
  for (final match in references.allMatches(value).toList().reversed) {
    final replacement = Platform.environment[match.group(1)];
    if (replacement == null) return null;
    expanded = expanded.replaceRange(match.start, match.end, replacement);
  }
  return expanded;
}

List<String> _stringList(Object? raw) =>
    raw is List ? raw.whereType<String>().toList() : <String>[];

String _environmentName(int serverIndex, String key) {
  final suffix = key.toUpperCase().replaceAll(RegExp(r'[^A-Z0-9_]'), '_');
  return 'ORGBOT_MCP_${serverIndex}_$suffix';
}

String _environmentReference(
  Map<String, String> environment,
  int serverIndex,
  String field,
  String value,
) {
  final name = _environmentName(serverIndex, field);
  environment[name] = value;
  return _tomlString('\${$name}');
}

String _environmentArray(
  Map<String, String> environment,
  int serverIndex,
  List<String> values,
) =>
    '[${List.generate(values.length, (index) => _environmentReference(environment, serverIndex, 'ARG_$index', values[index])).join(', ')}]';

String? _toolFilterReason(Map<String, Object?> entry) {
  const filteringKeys = {
    'tools',
    'enabled_tools',
    'disabled_tools',
    'allowed_tools',
    'denied_tools',
    'tools_allowlist',
    'tools_denylist',
    'default_tools_approval_mode',
    'tool_approval_mode',
    'approval_mode',
    'approval_policy',
  };
  String normalize(String value) => value
      .replaceAllMapped(
        RegExp(r'[A-Z]'),
        (match) => '_${match[0]!.toLowerCase()}',
      )
      .replaceFirst(RegExp(r'^_'), '');
  if (entry.keys.any(
        (key) => filteringKeys.contains(normalize(key.toString())),
      ) ||
      entry['_unsupported_tool_subtable'] == true ||
      entry['_unsupported_mcp_subtable'] == true) {
    return 'Per-tool restrictions cannot be represented for API-backed bots.';
  }
  return null;
}

String _stripTomlComment(String line) {
  var quoted = false;
  var escaped = false;
  for (var index = 0; index < line.length; index++) {
    final char = line[index];
    if (escaped) {
      escaped = false;
    } else if (quoted && char == r'\') {
      escaped = true;
    } else if (char == '"') {
      quoted = !quoted;
    } else if (char == '#' && !quoted) {
      return line.substring(0, index);
    }
  }
  return line;
}

String _tomlString(String value) => jsonEncode(value);
String _tomlKey(String key) =>
    RegExp(r'^[A-Za-z0-9_-]+$').hasMatch(key) ? key : _tomlString(key);
