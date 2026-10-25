import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/desktop_integrations.dart';

void main() {
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

      final snapshot = await DesktopIntegrations.discover(
        homeDirectory: home.path,
      );
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

      final snapshot = await DesktopIntegrations.discover(
        homeDirectory: home.path,
      );
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

      final snapshot = await DesktopIntegrations.discover(
        homeDirectory: home.path,
      );
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
}

Future<void> _skill(String path) async {
  await Directory(path).create(recursive: true);
  await File(
    '$path/SKILL.md',
  ).writeAsString('---\nname: sample\n---\nDo not load during discovery.\n');
}
