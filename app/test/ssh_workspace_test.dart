import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/workspace/workspace.dart';

void main() {
  final fixture = Platform.environment['ORGBOT_SSH_TEST_CONFIG'];
  test(
    'timeouts keep SSH alive; reads recover and interrupted commands are not replayed',
    () async {
      final info = jsonDecode(await File(fixture!).readAsString()) as Map;
      final connection = WorkspaceConnection(
        WorkspaceConfig(
          kind: 'ssh',
          host: info['host'],
          port: info['port'],
          username: 'qa',
          password: info['password'],
          fingerprint: info['fingerprint'],
          path: info['path'],
          commands: true,
        ),
        'recovery',
      );
      try {
        await connection.connect();
        await expectLater(
          connection.execute('workspace_run', {
            'command': 'sleep 2',
            'timeout_seconds': 1,
          }),
          throwsA(
            isA<StateError>().having(
              (e) => e.message,
              'connection kept',
              contains('SSH remains available'),
            ),
          ),
        );
        expect(connection.connected, isTrue);
        expect(
          await connection.execute('workspace_read', {'path': 'existing.txt'}),
          'original',
        );
        expect(
          await connection.execute('workspace_read', {
            'path': 'disconnect-once.txt',
          }),
          'read recovered',
        );
        await expectLater(
          connection.execute('workspace_run', {
            'command': 'orgbot-qa-disconnect',
          }),
          throwsStateError,
        );
        expect(
          await connection.execute('workspace_read', {
            'path': 'disconnect-marker',
          }),
          'once',
        );
        await expectLater(
          connection.execute('workspace_list', {'path': 'oversized'}),
          throwsFormatException,
        );
        expect(
          await connection.execute('workspace_read', {'path': 'existing.txt'}),
          'original',
        );
        final normal =
            await connection.execute('workspace_run', {
                  'command': 'printf still-connected',
                  'timeout_seconds': 1800,
                })
                as Map;
        expect(normal['output'], 'still-connected');
        if (Platform.environment['ORGBOT_SSH_LONG_CHECK'] == '1') {
          final long =
              await connection.execute('workspace_run', {
                    'command': 'sleep 61; printf long-command-finished',
                  })
                  as Map;
          expect(long['output'], 'long-command-finished');
          expect(connection.connected, isTrue);
        }
      } finally {
        await connection.close();
      }
    },
    timeout: const Timeout(Duration(minutes: 2)),
    skip: fixture == null ? 'Requires isolated SSH fixture.' : false,
  );
  test(
    'key-only SSH rejects passwords clearly and accepts an encrypted private key',
    () async {
      final info = jsonDecode(await File(fixture!).readAsString()) as Map;
      final config = WorkspaceConfig(
        kind: 'ssh',
        host: info['host'] as String,
        port: info['port'] as int,
        username: 'qa-key',
        path: info['path'] as String,
        password: info['password'] as String,
        fingerprint: info['fingerprint'] as String,
      );
      final password = WorkspaceConnection(config, 'password-rejected');
      await expectLater(
        password.connect(),
        throwsA(
          isA<StateError>().having(
            (e) => e.message,
            'explanation',
            contains('select Private key'),
          ),
        ),
      );
      await password.close();
      config.password = '';
      config.privateKey = await File(info['key_path'] as String).readAsString();
      config.passphrase = info['key_passphrase'] as String;
      final keyed = WorkspaceConnection(config, 'key-accepted');
      try {
        await keyed.connect();
        expect(
          await keyed.execute('workspace_read', {'path': 'existing.txt'}),
          'original',
        );
      } finally {
        await keyed.close();
      }
      config.passphrase = 'wrong-passphrase';
      final invalid = WorkspaceConnection(config, 'bad-passphrase');
      await expectLater(
        invalid.connect(),
        throwsA(
          isA<FormatException>().having(
            (e) => e.message,
            'explanation',
            contains('check its passphrase'),
          ),
        ),
      );
      await invalid.close();
    },
    skip: fixture == null ? 'Requires isolated SSH fixture.' : false,
  );
  test(
    'real SSH verifies identity and exercises file, shell, cancellation and permission boundaries',
    () async {
      final info = jsonDecode(await File(fixture!).readAsString()) as Map;
      final config = WorkspaceConfig(
        kind: 'ssh',
        host: info['host'] as String,
        port: info['port'] as int,
        username: 'qa',
        password: info['password'] as String? ?? 'orgbot-test-only',
        path: info['path'] as String,
        commands: true,
      );
      final workspace = WorkspaceConnection(config, 'ssh-test');
      final filename = 'ssh-check-${DateTime.now().microsecondsSinceEpoch}.txt';
      try {
        await workspace.connect(
          trust: (host, type, fingerprint) async {
            expect(fingerprint, info['fingerprint']);
            return true;
          },
        );
        await workspace.execute('workspace_write', {
          'path': filename,
          'content': 'first',
        });
        final initialMode =
            await workspace.execute('workspace_run', {
                  'command': 'stat -c %a $filename; chmod 700 $filename',
                })
                as Map;
        expect(initialMode['exit_code'], 0);
        expect(initialMode['output'], '600\n');
        await workspace.execute('workspace_write', {
          'path': filename,
          'content': 'second',
        });
        expect(
          await workspace.execute('workspace_read', {'path': filename}),
          'second',
        );
        final replacedMode =
            await workspace.execute('workspace_run', {
                  'command': 'stat -c %a $filename',
                })
                as Map;
        expect(replacedMode['output'], '700\n');
        final result =
            await workspace.execute('workspace_run', {
                  'command': 'printf "ssh-ok"',
                })
                as Map;
        expect(result['exit_code'], 0);
        expect(result['output'], 'ssh-ok');
        final pendingCommand = workspace.execute('workspace_run', {
          'command': 'printf once >> reconnect-marker; sleep 10',
        });
        final interrupted = expectLater(pendingCommand, throwsStateError);
        await Future<void>.delayed(const Duration(milliseconds: 300));
        await workspace.cancelOperation();
        await interrupted;
        expect(workspace.connected, isFalse);
        expect(
          await workspace.execute('workspace_read', {
            'path': 'reconnect-marker',
          }),
          'once',
        );
        expect(workspace.connected, isTrue);
        await workspace.execute('workspace_run', {
          'command': 'rm reconnect-marker',
        });
        await expectLater(
          workspace.execute('workspace_read', {'path': 'escape/outside.txt'}),
          throwsStateError,
        );
        await expectLater(
          workspace.execute('workspace_write', {
            'path': 'escape/outside.txt',
            'content': 'bad',
          }),
          throwsStateError,
        );
        config.writable = false;
        await expectLater(
          workspace.execute('workspace_run', {'command': 'echo denied'}),
          throwsStateError,
        );
        config.writable = true;
        final running = workspace.execute('workspace_run', {
          'command': 'sleep 3; printf late',
        });
        final cancelled = expectLater(running, throwsStateError);
        await Future<void>.delayed(const Duration(milliseconds: 150));
        await workspace.close();
        await cancelled;
        await expectLater(
          workspace.execute('workspace_read', {'path': filename}),
          throwsStateError,
        );
        final wrong = WorkspaceConnection(
          config.copy()..fingerprint = 'SHA256:wrong',
          'wrong',
        );
        await expectLater(wrong.connect(), throwsStateError);
        await wrong.close();
      } finally {
        await workspace.close();
      }
    },
    skip: fixture == null
        ? 'Set ORGBOT_SSH_TEST_CONFIG to the isolated fixture connection.json.'
        : false,
  );
}
