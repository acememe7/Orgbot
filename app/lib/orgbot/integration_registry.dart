import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:path_provider/path_provider.dart';

import '../engine/agent_cli.dart';

enum ManagedMcpTransport { stdio, http }

class ManagedMcpServer {
  const ManagedMcpServer({
    required this.id,
    required this.name,
    required this.enabled,
    required this.transport,
    this.command = '',
    this.args = const [],
    this.url = '',
    this.env = const {},
    this.headers = const {},
  });

  final String id;
  final String name;
  final bool enabled;
  final ManagedMcpTransport transport;
  final String command;
  final List<String> args;
  final String url;

  /// Values are environment variable names, never secret values.
  final Map<String, String> env;
  final Map<String, String> headers;

  Map<String, Object?> toJson() => {
    'id': id,
    'name': name,
    'enabled': enabled,
    'transport': transport.name,
    'command': command,
    'args': args,
    'url': url,
    'env': env,
    'headers': headers,
  };

  factory ManagedMcpServer.fromJson(Map<String, Object?> json) {
    final transport = ManagedMcpTransport.values
        .cast<ManagedMcpTransport?>()
        .firstWhere(
          (value) => value?.name == json['transport'],
          orElse: () => null,
        );
    if (transport == null ||
        json['id'] is! String ||
        json['name'] is! String ||
        json['enabled'] is! bool) {
      throw const FormatException('Invalid managed MCP registration.');
    }
    return ManagedMcpServer(
      id: json['id']! as String,
      name: json['name']! as String,
      enabled: json['enabled']! as bool,
      transport: transport,
      command: json['command'] is String ? json['command']! as String : '',
      args: json['args'] is List
          ? (json['args']! as List).whereType<String>().toList()
          : const [],
      url: json['url'] is String ? json['url']! as String : '',
      env: _stringMap(json['env']),
      headers: _stringMap(json['headers']),
    );
  }
}

/// User-managed MCP definitions and secrets. The JSON file contains only
/// non-secret configuration; credential values stay in the platform store.
class IntegrationRegistry {
  IntegrationRegistry({
    this._supportDirectory,
    FlutterSecureStorage? secureStorage,
  }) : _secureStorage = secureStorage ?? const FlutterSecureStorage();

  final String? _supportDirectory;
  final FlutterSecureStorage _secureStorage;
  Future<void> _writes = Future.value();

  static const _fileName = 'managed_integrations.json';
  static const _secretPrefix = 'orgbot_mcp_';
  static const _maxSkillBytes = 25 * 1024 * 1024;
  static const _maxSkillFiles = 300;

  Future<String> _directory() async =>
      _supportDirectory ?? (await getApplicationSupportDirectory()).path;

  Future<List<ManagedMcpServer>> listMcp() async {
    final file = File('${await _directory()}/$_fileName');
    if (!await file.exists()) return const [];
    if (await file.length() > 1024 * 1024) {
      throw const FormatException('Managed integrations file is too large.');
    }
    final decoded = jsonDecode(await file.readAsString());
    if (decoded is! Map || decoded['mcp'] is! List) {
      throw const FormatException('Invalid managed integrations file.');
    }
    return List.unmodifiable(
      (decoded['mcp']! as List).map((item) {
        if (item is! Map) {
          throw const FormatException('Invalid managed MCP registration.');
        }
        return ManagedMcpServer.fromJson(Map<String, Object?>.from(item));
      }),
    );
  }

  Future<void> saveMcp(
    ManagedMcpServer server, {
    Map<String, String> secrets = const {},
  }) => _serializeWrite(() async {
    _validate(server);
    final references = {...server.env.values, ...server.headers.values};
    if (secrets.keys.any((name) => !references.contains(name))) {
      throw ArgumentError('Every credential must have an MCP reference.');
    }
    for (final entry in secrets.entries) {
      final variable = entry.key;
      if (!RegExp(r'^[A-Za-z_][A-Za-z0-9_]*$').hasMatch(variable)) {
        throw ArgumentError.value(variable, 'secret name');
      }
      final value = entry.value;
      if (value.isEmpty) {
        await _secureStorage.delete(key: _secretKey(server.id, variable));
      } else {
        await _secureStorage.write(
          key: _secretKey(server.id, variable),
          value: value,
        );
      }
    }
    final existing = await listMcp();
    final previous = existing.where((item) => item.id == server.id);
    final staleSecrets = <String>{};
    if (previous.isNotEmpty) {
      final retained = {...server.env.values, ...server.headers.values};
      staleSecrets.addAll(
        {
          ...previous.first.env.values,
          ...previous.first.headers.values,
        }.difference(retained),
      );
    }
    await _write([...existing.where((item) => item.id != server.id), server]);
    for (final variable in staleSecrets) {
      await _secureStorage.delete(key: _secretKey(server.id, variable));
    }
  });

  Future<void> removeMcp(String id) => _serializeWrite(() async {
    final existing = await listMcp();
    final removed = existing.where((item) => item.id == id);
    await _write(existing.where((item) => item.id != id).toList());
    for (final server in removed) {
      for (final variable in {...server.env.values, ...server.headers.values}) {
        await _secureStorage.delete(key: _secretKey(id, variable));
      }
    }
  });

  Future<void> setMcpEnabled(String id, bool enabled) =>
      _serializeWrite(() async {
        final servers = await listMcp();
        final matches = servers.where((item) => item.id == id);
        final server = matches.isEmpty ? null : matches.first;
        if (server == null) throw StateError('MCP registration not found.');
        await _write([
          ...servers.where((item) => item.id != id),
          ManagedMcpServer(
            id: server.id,
            name: server.name,
            enabled: enabled,
            transport: server.transport,
            command: server.command,
            args: server.args,
            url: server.url,
            env: server.env,
            headers: server.headers,
          ),
        ]);
      });

  Future<String> installSkillDirectory(String sourcePath) async {
    final source = Directory(sourcePath);
    if (!await source.exists() ||
        await FileSystemEntity.type(source.path, followLinks: false) !=
            FileSystemEntityType.directory) {
      throw ArgumentError('Choose an existing skill folder.');
    }
    final name = _skillName(
      source.path
          .split(Platform.pathSeparator)
          .where((part) => part.isNotEmpty)
          .last,
    );
    return _installSkill(name, source);
  }

  Future<String> installSkillArchive(String archivePath) async {
    final archive = File(archivePath);
    if (!await archive.exists() || await archive.length() > _maxSkillBytes) {
      throw ArgumentError('Choose a ZIP archive smaller than 25 MiB.');
    }
    final members = (await _listZipMembers(
      archive.absolute.path,
    )).where((path) => path.isNotEmpty && !path.endsWith('/')).toList();
    if (members.isEmpty ||
        members.length > _maxSkillFiles ||
        members.toSet().length != members.length) {
      throw const FormatException(
        'The skill archive has an invalid file count.',
      );
    }
    for (final path in members) {
      _validateArchivePath(path);
    }
    final firstParts = members.map((path) => path.split('/').first).toSet();
    final prefix =
        firstParts.length == 1 &&
            members.every((path) => path.contains('/')) &&
            members.any((path) => path.endsWith('/SKILL.md'))
        ? '${firstParts.single}/'
        : '';
    final name = _skillName(
      archive.path
          .split(Platform.pathSeparator)
          .where((part) => part.isNotEmpty)
          .last
          .replaceFirst(RegExp(r'\.zip$', caseSensitive: false), ''),
    );
    final temporary = await Directory.systemTemp.createTemp('orgbot-skill-');
    try {
      var total = 0;
      for (final member in members) {
        final relative = prefix.isEmpty
            ? member
            : member.substring(prefix.length);
        if (relative.isEmpty) continue;
        _validateArchivePath(relative);
        final output = File('${temporary.path}/$relative');
        await output.parent.create(recursive: true);
        final bytes = await _readZipMember(archive.absolute.path, member);
        total += bytes.length;
        if (total > _maxSkillBytes) {
          throw const FormatException(
            'The expanded skill archive exceeds 25 MiB.',
          );
        }
        await output.writeAsBytes(bytes, flush: true);
      }
      return await _installSkill(name, temporary);
    } finally {
      if (await temporary.exists()) await temporary.delete(recursive: true);
    }
  }

  Future<void> removeSkill(String name) async {
    final root = Directory('${await _directory()}/managed_skills');
    if (await FileSystemEntity.type(root.path, followLinks: false) !=
        FileSystemEntityType.directory) {
      throw StateError('Managed skills folder is unavailable.');
    }
    final target = Directory('${root.path}/${_skillName(name)}');
    if (await FileSystemEntity.type(target.path, followLinks: false) !=
        FileSystemEntityType.directory) {
      throw StateError('Managed skill not found.');
    }
    await target.delete(recursive: true);
  }

  Future<String> managedSkillsDirectory() async =>
      '${await _directory()}/managed_skills';

  Future<String?> secret(String id, String variable) =>
      _secureStorage.read(key: _secretKey(id, variable));

  Future<void> _write(List<ManagedMcpServer> servers) async {
    final directory = await _directory();
    await Directory(directory).create(recursive: true);
    final file = File('$directory/$_fileName');
    final temporary = File('${file.path}.tmp');
    try {
      await temporary.writeAsString(
        jsonEncode({
          'version': 1,
          'mcp': servers.map((item) => item.toJson()).toList(),
        }),
        flush: true,
      );
      await temporary.rename(file.path);
    } finally {
      if (await temporary.exists()) await temporary.delete();
    }
  }

  Future<String> _installSkill(String name, Directory source) async {
    final manifest = File('${source.path}/SKILL.md');
    if (await FileSystemEntity.type(manifest.path, followLinks: false) !=
            FileSystemEntityType.file ||
        await manifest.length() == 0 ||
        await manifest.length() > 1024 * 1024) {
      throw ArgumentError('A skill needs a SKILL.md between 1 byte and 1 MiB.');
    }
    final root = Directory(await managedSkillsDirectory());
    await root.create(recursive: true);
    if (await FileSystemEntity.type(root.path, followLinks: false) !=
        FileSystemEntityType.directory) {
      throw StateError('Managed skills folder is unavailable.');
    }
    final target = Directory('${root.path}/$name');
    if (await target.exists()) {
      throw StateError('A skill with that name is installed.');
    }
    final stage = Directory(
      '${root.path}/.$name-${DateTime.now().microsecondsSinceEpoch}.tmp',
    );
    await stage.create();
    try {
      var count = 0;
      var total = 0;
      await for (final entity in source.list(
        recursive: true,
        followLinks: false,
      )) {
        final type = await FileSystemEntity.type(
          entity.path,
          followLinks: false,
        );
        if (type == FileSystemEntityType.link ||
            type == FileSystemEntityType.notFound) {
          throw ArgumentError('Skill folders cannot contain symbolic links.');
        }
        final relative = entity.path.substring(source.path.length + 1);
        final destination = '${stage.path}/$relative';
        if (type == FileSystemEntityType.directory) {
          await Directory(destination).create(recursive: true);
        } else if (type == FileSystemEntityType.file) {
          count++;
          final size = await File(entity.path).length();
          total += size;
          if (count > _maxSkillFiles || total > _maxSkillBytes) {
            throw ArgumentError('Skill folder exceeds its file or size limit.');
          }
          await File(entity.path).copy(destination);
        }
      }
      await stage.rename(target.path);
      return target.path;
    } catch (_) {
      if (await stage.exists()) await stage.delete(recursive: true);
      rethrow;
    }
  }

  static Future<List<int>> _readZipMember(String archive, String member) async {
    return _boundedZipCommand(
      ['-p', archive, member],
      _maxSkillBytes,
      'Skill archive extraction timed out.',
      'Could not safely extract a skill archive member.',
    );
  }

  static Future<List<String>> _listZipMembers(String archive) async {
    return utf8
        .decode(
          await _boundedZipCommand(
            ['-Z1', archive],
            1024 * 1024,
            'Reading the skill archive timed out.',
            'Could not read the skill ZIP archive.',
          ),
        )
        .split('\n');
  }

  static Future<List<int>> _boundedZipCommand(
    List<String> arguments,
    int maximumBytes,
    String timeoutMessage,
    String failureMessage,
  ) async {
    final process = await Process.start('unzip', arguments);
    final bytes = BytesBuilder(copy: false);
    var overflow = false;
    final stdoutDone = () async {
      await for (final chunk in process.stdout) {
        if (bytes.length + chunk.length > maximumBytes) {
          overflow = true;
          process.kill(ProcessSignal.sigkill);
        } else if (!overflow) {
          bytes.add(chunk);
        }
      }
    }();
    final stderrDone = process.stderr.drain<void>();
    try {
      final code = await process.exitCode.timeout(
        const Duration(seconds: 10),
        onTimeout: () {
          process.kill(ProcessSignal.sigkill);
          throw FormatException(timeoutMessage);
        },
      );
      await Future.wait([stdoutDone, stderrDone]).timeout(
        const Duration(seconds: 1),
      );
      if (overflow || code != 0) throw FormatException(failureMessage);
      return bytes.takeBytes();
    } catch (_) {
      process.kill(ProcessSignal.sigkill);
      try {
        await process.exitCode.timeout(const Duration(seconds: 1));
      } on Object {
        // The subprocess is killed above; do not block import cleanup forever.
      }
      rethrow;
    }
  }

  static void _validateArchivePath(String path) {
    if (path.startsWith('/') ||
        path.contains('\\') ||
        RegExp(r'^[A-Za-z]:').hasMatch(path) ||
        path
            .split('/')
            .any((part) => part.isEmpty || part == '.' || part == '..') ||
        path.codeUnits.any((unit) => unit < 0x20 || unit == 0x7f)) {
      throw const FormatException('The skill archive contains an unsafe path.');
    }
  }

  static String _skillName(String raw) {
    final cleaned = raw.trim().replaceAll(RegExp(r'[^A-Za-z0-9_-]+'), '-');
    if (!RegExp(r'^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$').hasMatch(cleaned)) {
      throw ArgumentError('Skill folder name is invalid.');
    }
    return cleaned;
  }

  Future<void> _serializeWrite(Future<void> Function() operation) {
    final next = _writes.then<void>(
      (_) => operation(),
      onError: (Object _) => operation(),
    );
    _writes = next;
    return next;
  }

  static String _secretKey(String id, String variable) =>
      '$_secretPrefix${base64Url.encode(utf8.encode('$id:$variable'))}';

  static void _validate(ManagedMcpServer server) {
    if (!RegExp(r'^[A-Za-z0-9_-]{1,64}$').hasMatch(server.id) ||
        server.name.trim().isEmpty ||
        server.name.length > 100) {
      throw ArgumentError('MCP name and id are invalid.');
    }
    if (server.transport == ManagedMcpTransport.stdio &&
        (server.command.trim().isEmpty ||
            server.command.length > 4096 ||
            server.args.length > 100 ||
            server.args.any((argument) => argument.length > 8192) ||
            server.env.length > 100)) {
      throw ArgumentError('A stdio MCP server needs a command.');
    }
    if (server.transport == ManagedMcpTransport.http) {
      final uri = Uri.tryParse(server.url);
      if (uri == null ||
          !uri.hasAuthority ||
          (uri.scheme != 'https' && uri.scheme != 'http')) {
        throw ArgumentError('MCP URL must use HTTP or HTTPS.');
      }
      if (uri.userInfo.isNotEmpty ||
          uri.hasFragment ||
          server.url.contains(r'${') ||
          server.url.length > 4096 ||
          server.headers.length > 100 ||
          uri.queryParameters.keys.any(
            (key) => RegExp(
              r'token|secret|password|api[_-]?key|authorization',
              caseSensitive: false,
            ).hasMatch(key),
          )) {
        throw ArgumentError(
          'Keep MCP credentials in secure header references.',
        );
      }
    }
    for (final variable in [...server.env.values, ...server.headers.values]) {
      if (!RegExp(r'^[A-Za-z_][A-Za-z0-9_]*$').hasMatch(variable)) {
        throw ArgumentError(
          'MCP credentials must reference environment names.',
        );
      }
    }
    for (final name in [...server.env.keys, ...server.headers.keys]) {
      if (name.length > 256 || name.contains('\n') || name.contains('\r')) {
        throw ArgumentError('MCP environment and header names are invalid.');
      }
    }
  }
}

Map<String, String> _stringMap(Object? value) => value is Map
    ? Map<String, String>.fromEntries(
        value.entries
            .where((entry) => entry.key is String && entry.value is String)
            .map(
              (entry) => MapEntry(entry.key as String, entry.value as String),
            ),
      )
    : const {};

enum ProviderPluginAction { install, update, remove }

typedef PluginCommandRunner =
    Future<ProcessResult> Function(String executable, List<String> arguments);

class ProviderPluginStatus {
  const ProviderPluginStatus({
    required this.provider,
    required this.executable,
    required this.actions,
    required this.commands,
    this.problem,
  });

  final CliKind provider;
  final String executable;
  final Set<ProviderPluginAction> actions;
  final Set<String> commands;
  final String? problem;
}

class ProviderPluginManager {
  ProviderPluginManager({PluginCommandRunner? runner})
    : _runner = runner ?? _run;

  final PluginCommandRunner _runner;

  Future<List<ProviderPluginStatus>> discover({
    String? home,
    String? path,
  }) async {
    final found = <ProviderPluginStatus>[];
    for (final provider in CliKind.values) {
      final cli = findCli(provider, home: home, path: path);
      final executable = cli.path;
      if (executable == null) continue;
      try {
        final result = await _runner(executable, [
          'plugin',
          '--help',
        ]).timeout(const Duration(seconds: 12));
        if (result.exitCode != 0) {
          found.add(
            ProviderPluginStatus(
              provider: provider,
              executable: executable,
              actions: const {},
              commands: const {},
              problem: 'Could not inspect plugin commands.',
            ),
          );
          continue;
        }
        final commands = _commands('${result.stdout}\n${result.stderr}');
        final actions = <ProviderPluginAction>{
          if (commands.intersection({'install', 'add'}).isNotEmpty)
            ProviderPluginAction.install,
          if (commands.intersection({'update', 'upgrade'}).isNotEmpty)
            ProviderPluginAction.update,
          if (commands.intersection({'remove', 'uninstall'}).isNotEmpty)
            ProviderPluginAction.remove,
        };
        found.add(
          ProviderPluginStatus(
            provider: provider,
            executable: executable,
            actions: Set.unmodifiable(actions),
            commands: Set.unmodifiable(commands),
            problem: actions.isEmpty
                ? 'No install, update, or remove command was reported.'
                : null,
          ),
        );
      } on TimeoutException {
        found.add(
          ProviderPluginStatus(
            provider: provider,
            executable: executable,
            actions: const {},
            commands: const {},
            problem: 'Plugin command discovery timed out.',
          ),
        );
      } on ProcessException {
        found.add(
          ProviderPluginStatus(
            provider: provider,
            executable: executable,
            actions: const {},
            commands: const {},
            problem: 'Could not run plugin command discovery.',
          ),
        );
      }
    }
    return List.unmodifiable(found);
  }

  Future<void> run(
    ProviderPluginStatus provider,
    ProviderPluginAction action,
    String plugin,
  ) async {
    final identifier = plugin.trim();
    if (!provider.actions.contains(action) ||
        identifier.isEmpty ||
        identifier.startsWith('-') ||
        identifier.length > 200 ||
        identifier.contains('\n')) {
      throw ArgumentError('Plugin action or identifier is invalid.');
    }
    final command = switch (action) {
      ProviderPluginAction.install => _findAction(provider, {'install', 'add'}),
      ProviderPluginAction.update => _findAction(provider, {
        'update',
        'upgrade',
      }),
      ProviderPluginAction.remove => _findAction(provider, {
        'remove',
        'uninstall',
      }),
    };
    final result = await _runner(provider.executable, [
      'plugin',
      command,
      identifier,
    ]).timeout(const Duration(minutes: 2));
    if (result.exitCode != 0) {
      throw StateError(
        'The provider CLI could not complete the plugin action.',
      );
    }
  }

  static Future<ProcessResult> _run(
    String executable,
    List<String> arguments,
  ) async {
    final process = await Process.start(
      executable,
      arguments,
      runInShell: false,
    );
    final stdout = _capture(process.stdout, process);
    final stderr = _capture(process.stderr, process);
    await process.stdin.close();
    final limit = arguments.contains('--help')
        ? const Duration(seconds: 10)
        : const Duration(minutes: 2);
    int exitCode;
    try {
      exitCode = await process.exitCode.timeout(limit);
    } on TimeoutException {
      process.kill(ProcessSignal.sigkill);
      exitCode = await process.exitCode;
    }
    final output = await Future.wait([stdout, stderr]);
    return ProcessResult(process.pid, exitCode, output[0], output[1]);
  }

  static Future<String> _capture(
    Stream<List<int>> stream,
    Process process,
  ) async {
    final bytes = BytesBuilder(copy: false);
    var overflow = false;
    await for (final chunk in stream) {
      if (bytes.length + chunk.length > 1024 * 1024) {
        overflow = true;
        process.kill(ProcessSignal.sigkill);
      } else if (!overflow) {
        bytes.add(chunk);
      }
    }
    return overflow ? '' : utf8.decode(bytes.takeBytes(), allowMalformed: true);
  }

  static Set<String> _commands(String help) {
    final section = RegExp(
      r'(?:^|\n)Commands:\s*\n(.*?)(?=\n\S|\nOptions:|\n$)',
      dotAll: true,
    ).firstMatch(help)?.group(1);
    if (section == null) return const {};
    return section
        .split('\n')
        .map(
          (line) => RegExp(
            r'^\s{2,}([a-z][a-z0-9_-]*(?:\|[a-z][a-z0-9_-]*)*)',
          ).firstMatch(line)?.group(1)?.split('|').first,
        )
        .whereType<String>()
        .toSet();
  }

  static String _findAction(
    ProviderPluginStatus provider,
    Set<String> alternatives,
  ) {
    final matches = provider.commands.intersection(alternatives);
    if (matches.isEmpty) {
      throw StateError('The provider command is unavailable.');
    }
    for (final option in alternatives) {
      if (matches.contains(option)) return option;
    }
    throw StateError('The provider command is unavailable.');
  }
}
