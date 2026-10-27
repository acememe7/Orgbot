import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/desktop_integrations.dart';
import 'package:orgbot_app/orgbot/integration_registry.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

Future<DesktopIntegrationSnapshot> _discover(Directory home) =>
    DesktopIntegrations.discover(
      homeDirectory: home.path,
      registry: IntegrationRegistry(supportDirectory: '${home.path}/support'),
    );

void main() {
  setUp(() => FlutterSecureStorage.setMockInitialValues({}));

  test(
    'discovers active direct MCP entries without exposing credentials',
    () async {
      final home = await Directory.systemTemp.createTemp(
        'orgbot-integrations-',
      );
      addTearDown(() => home.delete(recursive: true));
      await Directory('${home.path}/.codex').create(recursive: true);
      await File('${home.path}/.codex/config.toml').writeAsString('''
[mcp_servers.local_docs]
command = "node"
args = ["server.js", "private-arg-secret"]
[mcp_servers.local_docs.env]
API_TOKEN = "codex-secret-value"

[mcp_servers.disabled]
command = "hidden"
enabled = false
[mcp_servers.disabled.env]
TOKEN = "disabled-secret-value"

[mcp_servers.filtered]
command = "limited-tool-server"
enabled_tools = ["read_file"]

[mcp_servers.web]
url = "https://example.invalid/mcp?token=private-url-secret"
http_headers = { Authorization = "Bearer http-secret-value" }

[plugins."active@local-market"]
enabled = true
mcp_servers = { tools = [] }
''');
      await File('${home.path}/.claude.json').writeAsString(
        jsonEncode({
          'mcpServers': {
            'claude_local': {
              'command': 'python',
              'args': ['mcp.py'],
              'env': {'KEY': 'claude-secret-value'},
            },
            'oauth_remote': {
              'type': 'http',
              'url': 'https://example.invalid',
              'oauth': true,
            },
            'legacy_sse': {
              'type': 'sse',
              'url': 'https://example.invalid/events',
            },
          },
        }),
      );

      final snapshot = await _discover(home);
      expect(
        snapshot.mcpServers.map((server) => server.name),
        containsAll([
          'local_docs',
          'web',
          'claude_local',
          'oauth_remote',
          'legacy_sse',
          'filtered',
          'active@local-market',
        ]),
      );
      expect(
        snapshot.mcpServers.map((server) => server.name),
        isNot(contains('disabled')),
      );
      final oauth = snapshot.mcpServers.singleWhere(
        (server) => server.name == 'oauth_remote',
      );
      expect(oauth.state, DesktopIntegrationState.unsupported);
      expect(
        snapshot.mcpServers
            .singleWhere((server) => server.name == 'legacy_sse')
            .state,
        DesktopIntegrationState.unsupported,
      );
      expect(
        snapshot.mcpServers
            .singleWhere((server) => server.name == 'filtered')
            .state,
        DesktopIntegrationState.unsupported,
      );
      expect(
        snapshot.mcpServers
            .singleWhere((server) => server.name == 'active@local-market')
            .state,
        DesktopIntegrationState.unsupported,
      );
      expect(
        snapshot.mcpServers.every(
          (server) =>
              !server.detail.contains('secret-value') &&
              !server.detail.contains('API_TOKEN'),
        ),
        isTrue,
      );

      final environment = <String, String>{};
      final output = StringBuffer();
      snapshot.appendRoomConfig(output, environment: environment);
      final toml = output.toString();
      expect(toml, contains('transport = "stdio"'));
      expect(toml, contains('transport = "http"'));
      expect(toml, contains(r'${ORGBOT_MCP_'));
      expect(toml, isNot(contains('codex-secret-value')));
      expect(toml, isNot(contains('http-secret-value')));
      expect(toml, isNot(contains('claude-secret-value')));
      expect(toml, isNot(contains('private-arg-secret')));
      expect(toml, isNot(contains('private-url-secret')));
      expect(toml, isNot(contains('disabled-secret-value')));
      expect(environment.values, contains('codex-secret-value'));
      expect(environment.values, contains('Bearer http-secret-value'));
      expect(environment.values, contains('claude-secret-value'));
      expect(environment.values, contains('private-arg-secret'));
      expect(
        environment.values,
        contains('https://example.invalid/mcp?token=private-url-secret'),
      );
    },
  );

  test(
    'native discovery remains available without platform path provider',
    () async {
      final home = await Directory.systemTemp.createTemp('orgbot-headless-');
      addTearDown(() => home.delete(recursive: true));
      final snapshot = await DesktopIntegrations.discover(
        homeDirectory: home.path,
      );
      expect(snapshot.mcpServers, isEmpty);
      expect(snapshot.skills, isEmpty);
    },
  );

  test(
    'managed MCP secrets stay in secure storage and enter room env only',
    () async {
      final root = await Directory.systemTemp.createTemp('orgbot-managed-');
      addTearDown(() => root.delete(recursive: true));
      final home = Directory('${root.path}/home')..createSync();
      final registry = IntegrationRegistry(
        supportDirectory: '${root.path}/support',
      );
      await registry.saveMcp(
        const ManagedMcpServer(
          id: 'private_docs',
          name: 'Private docs',
          enabled: true,
          transport: ManagedMcpTransport.http,
          url: 'https://example.invalid/mcp',
          headers: {'Authorization': 'DOCS_TOKEN'},
        ),
        secrets: const {'DOCS_TOKEN': 'never-write-this'},
      );

      final registryFile = File(
        '${root.path}/support/managed_integrations.json',
      );
      expect(
        await registryFile.readAsString(),
        isNot(contains('never-write-this')),
      );
      final snapshot = await DesktopIntegrations.discover(
        homeDirectory: home.path,
        registry: registry,
      );
      final server = snapshot.mcpServers.single;
      expect(server.managed, isTrue);
      expect(server.state, DesktopIntegrationState.configured);
      expect(server.detail, isNot(contains('never-write-this')));
      final toml = StringBuffer();
      final environment = <String, String>{};
      snapshot.appendRoomConfig(toml, environment: environment);
      expect(
        toml.toString(),
        contains(r'${ORGBOT_MCP_0_HEADER_AUTHORIZATION}'),
      );
      expect(toml.toString(), isNot(contains('never-write-this')));
      expect(environment.values, contains('never-write-this'));
    },
  );

  test(
    'managed skills install atomically, appear in discovery, and remove',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'orgbot-skill-managed-',
      );
      addTearDown(() => root.delete(recursive: true));
      final source = Directory('${root.path}/source')..createSync();
      final home = Directory('${root.path}/home')..createSync();
      await File(
        '${source.path}/SKILL.md',
      ).writeAsString('Use the release checklist.');
      final registry = IntegrationRegistry(
        supportDirectory: '${root.path}/support',
      );

      final installed = await registry.installSkillDirectory(source.path);
      expect(
        File('$installed/SKILL.md').readAsStringSync(),
        contains('release checklist'),
      );
      final snapshot = await DesktopIntegrations.discover(
        homeDirectory: home.path,
        registry: registry,
      );
      expect(snapshot.skills.single.source, DesktopIntegrationSource.managed);
      await registry.removeSkill('source');
      expect(await Directory(installed).exists(), isFalse);
    },
  );

  test(
    'managed Streamable HTTP test sends credential and lists tools',
    () async {
      final root = await Directory.systemTemp.createTemp('orgbot-mcp-probe-');
      addTearDown(() => root.delete(recursive: true));
      final registry = IntegrationRegistry(
        supportDirectory: '${root.path}/support',
      );
      String? receivedAuthorization;
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      addTearDown(() => server.close(force: true));
      server.listen((request) async {
        receivedAuthorization = request.headers.value(
          HttpHeaders.authorizationHeader,
        );
        final message =
            jsonDecode(await utf8.decoder.bind(request).join()) as Map;
        final method = message['method'];
        if (method == 'initialize') {
          request.response.headers.contentType = ContentType.json;
          request.response.write(
            jsonEncode({
              'jsonrpc': '2.0',
              'id': message['id'],
              'result': {'protocolVersion': '2025-03-26'},
            }),
          );
        } else if (method == 'tools/list') {
          request.response.headers.contentType = ContentType.json;
          request.response.write(
            jsonEncode({
              'jsonrpc': '2.0',
              'id': message['id'],
              'result': {
                'tools': [
                  {'name': 'search'},
                ],
              },
            }),
          );
        } else {
          request.response.statusCode = HttpStatus.accepted;
        }
        await request.response.close();
      });
      final managed = ManagedMcpServer(
        id: 'http_fixture',
        name: 'HTTP fixture',
        enabled: true,
        transport: ManagedMcpTransport.http,
        url: 'http://127.0.0.1:${server.port}/mcp',
        headers: const {'Authorization': 'FIXTURE_TOKEN'},
      );
      await registry.saveMcp(
        managed,
        secrets: const {'FIXTURE_TOKEN': 'private-token'},
      );

      final result = await DesktopIntegrations.testManaged(managed, registry);
      expect(result, 'Connected · 1 tools');
      expect(receivedAuthorization, 'private-token');
      expect(result, isNot(contains('private-token')));

      final snapshot = await DesktopIntegrations.discover(
        homeDirectory: root.path,
        registry: registry,
      );
      final validation = await snapshot.validateConnections();
      expect(validation.configured, 1);
      expect(validation.reachable, 1);
      expect(validation.unavailable, 0);
    },
  );

  test(
    'managed stdio test starts an isolated process with secure env',
    () async {
      final root = await Directory.systemTemp.createTemp('orgbot-mcp-stdio-');
      addTearDown(() => root.delete(recursive: true));
      final script = File('${root.path}/server.sh')
        ..writeAsStringSync('''
#!/bin/sh
read line
if [ "\$AUTH_TOKEN" != "fixture-token" ]; then exit 5; fi
printf '%s\\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26"}}'
read line
read line
printf '%s\\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"search"}]}}'
''');
      final registry = IntegrationRegistry(
        supportDirectory: '${root.path}/support',
      );
      final managed = ManagedMcpServer(
        id: 'stdio_fixture',
        name: 'stdio fixture',
        enabled: true,
        transport: ManagedMcpTransport.stdio,
        command: '/bin/sh',
        args: [script.path],
        env: const {'AUTH_TOKEN': 'STDIO_TOKEN'},
      );
      await registry.saveMcp(
        managed,
        secrets: const {'STDIO_TOKEN': 'fixture-token'},
      );

      expect(
        await DesktopIntegrations.testManaged(managed, registry),
        'Connected · 1 tools',
      );
    },
  );

  test('skill ZIP import installs safe paths and rejects traversal', () async {
    final root = await Directory.systemTemp.createTemp('orgbot-skill-zip-');
    addTearDown(() => root.delete(recursive: true));
    final safeZip = '${root.path}/release-guide.zip';
    final badZip = '${root.path}/unsafe.zip';
    final linkedZip = '${root.path}/linked.zip';
    final zipped = await Process.run('python3', [
      '-c',
      'import stat, sys, zipfile; z=zipfile.ZipFile(sys.argv[1], "w"); z.writestr("release/SKILL.md", "Review each release gate."); z.close(); z=zipfile.ZipFile(sys.argv[2], "w"); z.writestr("../escape.txt", "outside"); z.writestr("unsafe/SKILL.md", "unsafe"); z.close(); z=zipfile.ZipFile(sys.argv[3], "w"); i=zipfile.ZipInfo("linked/link"); i.create_system=3; i.external_attr=(stat.S_IFLNK | 0o777) << 16; z.writestr(i, "../../escape.txt"); z.writestr("linked/SKILL.md", "linked skill"); z.close()',
      safeZip,
      badZip,
      linkedZip,
    ]);
    expect(zipped.exitCode, 0);
    final registry = IntegrationRegistry(
      supportDirectory: '${root.path}/support',
    );

    final installed = await registry.installSkillArchive(safeZip);
    expect(
      File('$installed/SKILL.md').readAsStringSync(),
      contains('release gate'),
    );
    await expectLater(
      registry.installSkillArchive(badZip),
      throwsFormatException,
    );
    expect(File('${root.path}/escape.txt').existsSync(), isFalse);
    expect(
      await Directory('${root.path}/support/managed_skills/unsafe').exists(),
      isFalse,
    );
    final linked = await registry.installSkillArchive(linkedZip);
    final linkEntry = File('$linked/link');
    expect(await linkEntry.readAsString(), '../../escape.txt');
    expect(
      await FileSystemEntity.type(linkEntry.path, followLinks: false),
      FileSystemEntityType.file,
    );
    expect(File('${root.path}/escape.txt').existsSync(), isFalse);

    final source = Directory('${root.path}/source')..createSync();
    await File('${source.path}/SKILL.md').writeAsString('linked source');
    await Link('${source.path}/link').create('${root.path}/escape.txt');
    await expectLater(
      registry.installSkillDirectory(source.path),
      throwsArgumentError,
    );
  });

  test(
    'discovers only installed skill folders and enabled Claude plugin skills',
    () async {
      final home = await Directory.systemTemp.createTemp('orgbot-skills-');
      addTearDown(() => home.delete(recursive: true));
      await _skill('${home.path}/.agents/skills/plan');
      await _skill(
        '${home.path}/.claude/plugins/marketplaces/local/plugin/skills/review',
      );
      await Directory(
        '${home.path}/.claude/skills/uninstalled',
      ).create(recursive: true);
      await _skill(
        '${home.path}/.codex/plugins/cache/local/plugin/1.0.0/skills/plugin-skill',
      );
      await Directory('${home.path}/.codex').create(recursive: true);
      await File('${home.path}/.codex/config.toml').writeAsString('''
[plugins."plugin@local"]
enabled = true

[plugins."disabled@local"]
enabled = false
''');
      await File('${home.path}/.claude/settings.json').writeAsString(
        jsonEncode({
          'enabledPlugins': {'plugin@local': true, 'disabled@local': false},
        }),
      );
      await File(
        '${home.path}/.claude/plugins/installed_plugins.json',
      ).writeAsString(
        jsonEncode({
          'plugins': {
            'plugin@local': [
              {
                'installPath':
                    '${home.path}/.claude/plugins/marketplaces/local/plugin',
              },
            ],
            'disabled@local': [
              {'installPath': '${home.path}/.claude/plugins/disabled'},
            ],
          },
        }),
      );

      final snapshot = await _discover(home);
      expect(
        snapshot.skills.map((skill) => skill.name),
        containsAll(['plan', 'review', 'plugin-skill']),
      );
      expect(
        snapshot.skills.map((skill) => skill.name),
        isNot(contains('uninstalled')),
      );
      expect(
        snapshot.skills.map((skill) => skill.name),
        isNot(contains('disabled')),
      );
    },
  );
  test(
    'Codex MCP per-tool policies are status-only and never imported',
    () async {
      final home = await Directory.systemTemp.createTemp('orgbot-tool-policy-');
      addTearDown(() => home.delete(recursive: true));
      await Directory('${home.path}/.codex').create(recursive: true);
      await File('${home.path}/.codex/config.toml').writeAsString('''
[mcp_servers.subtable_policy]
command = "restricted-server"
args = ["--safe"]

[mcp_servers.subtable_policy.tools.remove_file]
approval_mode = "deny"

[mcp_servers.inline_policy]
command = "filtered-server"
tools = { remove_file = { enabled = false } }
''');

      final snapshot = await _discover(home);
      for (final name in ['subtable_policy', 'inline_policy']) {
        expect(
          snapshot.mcpServers
              .singleWhere((server) => server.name == name)
              .state,
          DesktopIntegrationState.unsupported,
        );
      }
      final environment = <String, String>{};
      final room = StringBuffer();
      snapshot.appendRoomConfig(room, environment: environment);
      expect(room.toString(), isNot(contains('restricted-server')));
      expect(room.toString(), isNot(contains('filtered-server')));
      expect(environment, isEmpty);
    },
  );

  test('plugin commands are discovered from CLI help before execution', () async {
    final root = await Directory.systemTemp.createTemp('orgbot-plugin-cli-');
    addTearDown(() => root.delete(recursive: true));
    final bin = Directory('${root.path}/bin')..createSync();
    for (final name in ['codex', 'claude', 'grok', 'opencode', 'agy']) {
      final cli = File('${bin.path}/$name')..writeAsStringSync('fixture');
      await Process.run('chmod', ['+x', cli.path]);
    }
    final help = {
      'codex':
          'Commands:\n  add Install\n  list List\n  remove Remove\n\nOptions:\n',
      'claude':
          'Commands:\n  install|i Install\n  update Update\n  uninstall|remove Remove\n\nOptions:\n',
      'grok':
          'Commands:\n  install Install\n  update Update\n  uninstall Remove\n\nOptions:\n',
    };
    final calls = <List<String>>[];
    final invoked = <String>[];
    final manager = ProviderPluginManager(
      runner: (executable, arguments) async {
        calls.add(arguments);
        final provider = executable.split('/').last;
        invoked.add(provider);
        return arguments.length == 2
            ? ProcessResult(1, 0, help[provider]!, '')
            : ProcessResult(1, 0, '', '');
      },
    );
    final providers = await manager.discover(home: root.path, path: bin.path);
    expect(invoked, isNot(contains('opencode')));
    expect(invoked, isNot(contains('agy')));
    final codex = providers.singleWhere(
      (provider) => provider.provider.name == 'codex',
    );
    final claude = providers.singleWhere(
      (provider) => provider.provider.name == 'claude',
    );
    final grok = providers.singleWhere(
      (provider) => provider.provider.name == 'grok',
    );
    expect(providers.length, 3);
    expect(
      codex.actions,
      containsAll([ProviderPluginAction.install, ProviderPluginAction.remove]),
    );
    expect(codex.actions, isNot(contains(ProviderPluginAction.update)));
    await manager.run(codex, ProviderPluginAction.remove, 'local@marketplace');
    expect(calls.last, ['plugin', 'remove', 'local@marketplace']);
    await manager.run(
      claude,
      ProviderPluginAction.update,
      'plugin@marketplace',
    );
    expect(calls.last, ['plugin', 'update', 'plugin@marketplace']);
    await manager.run(
      grok,
      ProviderPluginAction.install,
      'https://example.invalid/plugin',
    );
    expect(calls.last, ['plugin', 'install', 'https://example.invalid/plugin']);
    await expectLater(
      manager.run(codex, ProviderPluginAction.remove, '--help'),
      throwsArgumentError,
    );
  });
}

Future<void> _skill(String path) async {
  await Directory(path).create(recursive: true);
  await File(
    '$path/SKILL.md',
  ).writeAsString('---\nname: sample\n---\nDo not load during discovery.\n');
}
