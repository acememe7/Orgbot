// Bots backed by the machine's agent CLIs, which carry the sign-in.
//
// The argv here has to match what each CLI actually accepts — a wrong flag is
// an exit code, not a compile error — so it is asserted directly, as is the
// parsing of each CLI's event stream.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/engine/agent_cli.dart';
import 'package:orgbot_app/orgbot/models.dart';

void main() {
  group('CLI discovery', () {
    test(
      'matches the core search order and supported home directories',
      () async {
        if (Platform.isWindows) return;
        final root = await Directory.systemTemp.createTemp('orgbot-cli-test-');
        try {
          final local = await _executable(root, 'home/.local/bin/grok');
          final npm = await _executable(root, 'home/.npm-global/bin/grok');
          final grok = await _executable(root, 'home/.grok/bin/grok');
          final bun = await _executable(root, 'home/.bun/bin/grok');
          final pathBinary = await _executable(root, 'path-bin/grok');
          final home = '${root.path}/home';

          expect(findCli(CliKind.grok, path: '', home: home).path, local.path);
          expect(
            findCli(
              CliKind.grok,
              path: pathBinary.parent.path,
              home: home,
            ).path,
            pathBinary.path,
          );

          await local.delete();
          expect(findCli(CliKind.grok, path: '', home: home).path, npm.path);

          await npm.delete();
          expect(findCli(CliKind.grok, path: '', home: home).path, grok.path);

          await grok.delete();
          expect(
            findCli(CliKind.grok, path: '', home: home).available,
            isFalse,
          );
          expect(bun.existsSync(), isTrue);
        } finally {
          await root.delete(recursive: true);
        }
      },
    );

    test('finds the Linux protected CLI binaries', () async {
      if (!Platform.isLinux) return;
      final root = await Directory.systemTemp.createTemp('orgbot-cli-test-');
      try {
        final opencode = await _executable(root, 'home/.opencode/bin/opencode');
        final antigravity = await _executable(root, 'home/.local/bin/agy');
        final home = '${root.path}/home';
        expect(
          findCli(CliKind.opencode, path: '', home: home).path,
          opencode.path,
        );
        expect(
          findCli(CliKind.antigravity, path: '', home: home).path,
          antigravity.path,
        );
        expect(CliKind.fromBinary('agy'), CliKind.antigravity);
      } finally {
        await root.delete(recursive: true);
      }
    });

    test('does not treat an executable directory as a CLI binary', () async {
      if (Platform.isWindows) return;
      final root = await Directory.systemTemp.createTemp('orgbot-cli-test-');
      try {
        final binaryDirectory = Directory('${root.path}/codex');
        await binaryDirectory.create();
        final status = findCli(CliKind.codex, path: root.path, home: '');
        expect(status.available, isFalse);
      } finally {
        await root.delete(recursive: true);
      }
    });
  });

  group('argv', () {
    test('a fresh codex session asks for JSON and skips the git check', () {
      final args = buildArgs(cli: CliKind.codex, prompt: 'hello');
      expect(args, ['exec', '--json', '--skip-git-repo-check', 'hello']);
    });

    test('a resumed codex session passes the id before the prompt', () {
      final args = buildArgs(
        cli: CliKind.codex,
        prompt: 'more',
        resumed: 'sess_1',
      );
      // Options must precede the positionals, and the id comes before the
      // prompt or codex reads them the wrong way round.
      expect(args, [
        'exec',
        'resume',
        '--json',
        '--skip-git-repo-check',
        'sess_1',
        'more',
      ]);
    });

    test('claude streams JSON and resumes by id', () {
      final fresh = buildArgs(cli: CliKind.claude, prompt: 'hi');
      expect(fresh, contains('-p'));
      expect(fresh, containsAllInOrder(['--output-format', 'stream-json']));
      expect(fresh, contains('--verbose'));
      expect(fresh, isNot(contains('--resume')));

      final resumed = buildArgs(
        cli: CliKind.claude,
        prompt: 'hi',
        resumed: 'abc',
      );
      expect(resumed, containsAllInOrder(['--resume', 'abc']));
    });

    test('grok uses its own stream format and -r to resume', () {
      final fresh = buildArgs(cli: CliKind.grok, prompt: 'hi');
      expect(
        fresh,
        containsAllInOrder(['--output-format', 'streaming-messages-json']),
      );
      final resumed = buildArgs(
        cli: CliKind.grok,
        prompt: 'hi',
        resumed: 'xyz',
      );
      expect(resumed, containsAllInOrder(['-r', 'xyz']));
    });

    test('an empty model is left off so the subscription default applies', () {
      expect(buildArgs(cli: CliKind.grok, prompt: 'hi'), isNot(contains('-m')));
      expect(
        buildArgs(cli: CliKind.grok, prompt: 'hi', model: 'grok-4.6'),
        containsAllInOrder(['-m', 'grok-4.6']),
      );
    });

    test('protected CLI args are rejected by the legacy Dart runner', () {
      for (final cli in [CliKind.opencode, CliKind.antigravity]) {
        expect(
          () => buildArgs(cli: cli, prompt: 'hello'),
          throwsA(
            isA<UnsupportedError>().having(
              (error) => error.message,
              'message',
              contains('requires the protected Linux runtime'),
            ),
          ),
        );
      }
    });

    test(
      'legacy CLI bots reject protected providers before process launch',
      () async {
        for (final cli in [CliKind.opencode, CliKind.antigravity]) {
          var launched = false;
          final bot = AgentCliBot(
            cli: cli,
            binPath: cli.binary,
            handle: 'test',
            system: 'test',
            runner: (_, _) async {
              launched = true;
              return const CliResult(0, '', '');
            },
          );
          await expectLater(
            bot.act(situation: 'test'),
            throwsA(isA<UnsupportedError>()),
          );
          expect(launched, isFalse);
        }
      },
    );
  });

  group('event parsing', () {
    test('codex reports its session and final message', () {
      const stdout = '''
{"type":"thread.started","thread_id":"th_42"}
{"type":"item.started","item":{"type":"reasoning"}}
{"type":"item.completed","item":{"type":"agent_message","text":"{\\"actions\\":[]}"}}
''';
      final episode = parseEpisode(CliKind.codex, stdout);
      expect(episode.sessionId, 'th_42');
      expect(episode.text, contains('actions'));
    });

    test('claude and grok both yield the result line', () {
      const stdout = '''
{"type":"system","subtype":"init","session_id":"s_9"}
{"type":"assistant","message":{"content":[{"type":"text","text":"partial"}]}}
{"type":"result","subtype":"success","session_id":"s_9","result":"final answer"}
''';
      for (final cli in [CliKind.claude, CliKind.grok]) {
        final episode = parseEpisode(cli, stdout);
        expect(
          episode.sessionId,
          's_9',
          reason: '$cli should read the session',
        );
        expect(episode.text, contains('final answer'));
      }
    });

    test('banner lines that are not JSON are skipped, not fatal', () {
      const stdout = '''
Welcome to the CLI!
{"type":"result","result":"ok"}
not json either
''';
      expect(parseEpisode(CliKind.claude, stdout).text, contains('ok'));
    });

    test('protected CLI streams are handled only by the Linux runtime', () {
      for (final cli in [CliKind.opencode, CliKind.antigravity]) {
        expect(() => parseEpisode(cli, '{}'), throwsA(isA<UnsupportedError>()));
      }
    });
  });

  group('reply parsing', () {
    test('a bare JSON object becomes tool calls', () {
      final turn = parseAgentReply(
        '{"say":"on it","actions":[{"tool":"claim_task","args":{"task":"task_1"}}]}',
      );
      expect(turn.text, 'on it');
      expect(turn.tools.single.name, 'claim_task');
      expect(turn.tools.single.args['task'], 'task_1');
    });

    test('an object wrapped in prose or a fence is still found', () {
      final turn = parseAgentReply(
        'Sure thing:\n```json\n{"actions":[{"tool":"room_status"}]}\n```\nHope that helps.',
      );
      expect(turn.tools.single.name, 'room_status');
    });

    /// A brace inside a message must not truncate the object.
    test('braces inside strings do not confuse extraction', () {
      final turn = parseAgentReply(
        '{"say":"use {curly} braces","actions":[{"tool":"post_message","args":{"body":"hi"}}]}',
      );
      expect(turn.text, 'use {curly} braces');
      expect(turn.tools.single.name, 'post_message');
    });

    test('a reply with no JSON becomes something the bot says', () {
      final turn = parseAgentReply('I think we should start with the schema.');
      expect(turn.tools, isEmpty);
      expect(turn.text, 'I think we should start with the schema.');
    });

    test('an empty action list is a turn that does nothing', () {
      final turn = parseAgentReply('{"actions":[]}');
      expect(turn.tools, isEmpty);
      expect(turn.text, isNull);
    });

    test('a malformed action is skipped rather than failing the turn', () {
      final turn = parseAgentReply(
        '{"actions":[{"noname":true},{"tool":"list_tasks","args":{}}]}',
      );
      expect(turn.tools.single.name, 'list_tasks');
    });
  });

  _realStreamTests();

  group('provider wiring', () {
    test('CLI providers avoid API keys and preserve device-sign-in rules', () {
      expect(ProviderKind.anthropic.cli, CliKind.claude);
      expect(ProviderKind.codex.cli, CliKind.codex);
      expect(ProviderKind.grok.cli, CliKind.grok);
      expect(ProviderKind.opencode.cli, CliKind.opencode);
      expect(ProviderKind.antigravity.cli, CliKind.antigravity);

      for (final (p, deviceSignIn) in [
        (ProviderKind.anthropic, false),
        (ProviderKind.codex, true),
        (ProviderKind.grok, true),
        (ProviderKind.opencode, false),
        (ProviderKind.antigravity, false),
      ]) {
        expect(p.usesSignIn, isTrue, reason: '$p should use a CLI sign-in');
        expect(p.needsApiKey, isFalse, reason: '$p must not ask for a key');
        expect(p.signsInOnDevice, deviceSignIn);
      }
      expect(ProviderKind.deepseek.needsApiKey, isTrue);
      expect(ProviderKind.deepseek.usesSignIn, isFalse);
      expect(ProviderKind.scripted.needsApiKey, isFalse);
    });

    test('grok is offered as a provider again', () {
      expect(ProviderKind.values.map((p) => p.key), contains('grok'));
      expect(ProviderKind.fromKey('opencode'), ProviderKind.opencode);
      expect(ProviderKind.fromKey('antigravity'), ProviderKind.antigravity);
      expect(ProviderKind.fromKey('agy'), ProviderKind.scripted);
    });

    /// Bots saved before the rename used `openai` for what is now Codex.
    test('a bot saved under the old openai key still loads', () {
      expect(ProviderKind.fromKey('openai'), ProviderKind.codex);
    });

    test('a bot only needs a key when its provider takes one', () {
      final claude = BotDef(name: 'a', provider: ProviderKind.anthropic);
      expect(
        claude.needsKey,
        isFalse,
        reason: 'a signed-in CLI bot must never be flagged as missing a key',
      );

      final deepseek = BotDef(name: 'b', provider: ProviderKind.deepseek);
      expect(deepseek.needsKey, isTrue);
      expect(
        BotDef(
          name: 'c',
          provider: ProviderKind.deepseek,
          apiKey: 'sk-1',
        ).needsKey,
        isFalse,
      );
    });
  });
}

Future<File> _executable(Directory root, String relativePath) async {
  final file = File('${root.path}/$relativePath');
  await file.parent.create(recursive: true);
  await file.writeAsString('#!/bin/sh\nexit 0\n');
  final result = await Process.run('chmod', ['+x', file.path]);
  if (result.exitCode != 0) {
    throw StateError('Could not make the CLI fixture executable.');
  }
  return file;
}

/// Captured from a real `grok -p ... --output-format streaming-messages-json`
/// run on this machine. Keeping the genuine shape here is what stops the
/// parser drifting from what the CLI actually emits — the failure mode that
/// unit-only tests never catch.
const String _realGrokStream = r'''
{"type":"system","subtype":"init","session_id":"01a0687d-2ee9-7bd2-9436-6093614ce560","apiKeySource":"oauth","model":"grok-4.6","cwd":"/tmp","permissionMode":"default"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"The user wants a JSON object."},{"type":"text","text":"{\"say\":\"probe ok\",\"actions\":[]}"}]}}
{"type":"result","subtype":"success","is_error":false,"duration_ms":3011,"num_turns":1,"result":"{\"say\":\"probe ok\",\"actions\":[]}","stop_reason":"end_turn","usage":{"input_tokens":10133,"output_tokens":70}}
''';

void _realStreamTests() {
  group('against real CLI output', () {
    test('a genuine grok stream yields the session and the agent JSON', () {
      final episode = parseEpisode(CliKind.grok, _realGrokStream);
      expect(episode.sessionId, '01a0687d-2ee9-7bd2-9436-6093614ce560');

      final turn = parseAgentReply(episode.text);
      expect(turn.text, 'probe ok');
      expect(turn.tools, isEmpty);
    });

    /// The init line reports how the CLI authenticated. `oauth` is the whole
    /// point of this backend: the subscription is used, no key is involved.
    test('the CLI authenticates by sign-in, not an API key', () {
      expect(_realGrokStream, contains('"apiKeySource":"oauth"'));
    });
  });
}
