// Run against the isolated SSH fixture described in README; no real model keys.
import 'dart:io';
import 'dart:convert';
import 'dart:ui' as ui;
import 'package:flutter/rendering.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'package:orgbot_app/state/room_archive.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:orgbot_app/engine/embedded_engine.dart';
import 'package:orgbot_app/workspace/workspace.dart';
import 'package:orgbot_app/workspace/workspace_screen.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('standalone native engine and phone file tools work on Android', (
    tester,
  ) async {
    final base = await WorkspaceConnection.phoneBase();
    final dir = Directory(
      '$base/device-qa-${DateTime.now().microsecondsSinceEpoch}',
    );
    await dir.create(recursive: true);
    final local = WorkspaceConnection(
      WorkspaceConfig(path: dir.path),
      'device-qa',
    );
    try {
      await local.connect();
      await local.execute('workspace_write', {
        'path': 'native.txt',
        'content': 'written on Android',
      });
      expect(
        await local.execute('workspace_read', {'path': 'native.txt'}),
        'written on Android',
      );
      expect((await File('${dir.path}/native.txt').stat()).mode & 0x1ff, 0x180);
      await workspaceChannel.invokeMethod<void>('chmod', {
        'path': '${dir.path}/native.txt',
        'mode': 0x1c0,
      });
      await local.execute('workspace_write', {
        'path': 'native.txt',
        'content': 'written on Android',
      });
      expect((await File('${dir.path}/native.txt').stat()).mode & 0x1ff, 0x1c0);
      await expectLater(
        local.execute('workspace_run', {'command': 'echo denied'}),
        throwsStateError,
      );
      final engine = EmbeddedEngine.load();
      final handle = engine.openRoom('''
[room]
name = "device QA"
goal = "standalone native test"
completion = "manual"
[[bots]]
handle = "demo"
provider = { kind = "scripted", script = "Native engine answered." }
''', {});
      Future<void> phase(String wanted) async {
        for (var i = 0; i < 100; i++) {
          engine.poll(handle);
          if ((engine.snapshot(handle)?['room'] as Map?)?['phase'] == wanted) {
            return;
          }
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
        fail('Native room did not reach $wanted');
      }

      try {
        expect(
          engine.control(handle, {'type': 'pause', 'reason': 'device test'}),
          isTrue,
        );
        await phase('paused');
        expect(engine.control(handle, {'type': 'resume'}), isTrue);
        await phase('running');
        expect(engine.say(handle, 'QA', '@demo Please answer'), isTrue);
        var answered = false;
        var directed = false;
        for (var i = 0; i < 100; i++) {
          final messages = engine.poll(handle);
          for (final message in messages) {
            final event = (message['envelope'] as Map?)?['event'] as Map?;
            final posted = event?['message'] as Map?;
            if (posted?['body'] == '@demo Please answer') {
              directed = (posted?['audience'] as Map?)?['kind'] == 'agents';
            }
          }
          if (messages.toString().contains('Native engine answered.')) {
            answered = true;
            break;
          }
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
        expect(answered, isTrue);
        expect(directed, isTrue);
      } finally {
        engine.closeRoom(handle);
      }
      await tester.pumpWidget(
        MaterialApp(
          home: WorkspaceFilesScreen(
            config: WorkspaceConfig(path: dir.path),
            roomId: 'device-qa',
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('native.txt'), findsOneWidget);
      await tester.tap(find.text('native.txt'));
      await tester.pumpAndSettle();
      expect(find.text('written on Android'), findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpAndSettle();
    } finally {
      await local.close();
      await dir.delete(recursive: true);
    }
  });
  testWidgets('saved Android room UI pauses, resumes and keeps a transcript', (
    tester,
  ) async {
    final library = Library(saver: MemorySaver());
    await library.load();
    final bot = library.addBot(
      BotDef(name: 'QA demo', provider: ProviderKind.scripted),
    );
    final room = library.addRoom(
      RoomDef(
        name: 'Device QA room',
        goal: 'Chat with the demo',
        bots: [RoomSlot(bot.id)],
      ),
    );
    final capture = GlobalKey();
    await tester.pumpWidget(
      RepaintBoundary(
        key: capture,
        child: MaterialApp(home: OrgBotScreen(library: library)),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Device QA room'));
    await tester.pumpAndSettle();
    for (var i = 0; i < 50 && find.byTooltip('Pause').evaluate().isEmpty; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(find.byTooltip('Pause'), findsOneWidget);
    await tester.tap(find.byTooltip('Pause'));
    await tester.pump(const Duration(milliseconds: 500));
    expect(find.byTooltip('Resume'), findsOneWidget);
    await tester.tap(find.byTooltip('Resume'));
    await tester.pump(const Duration(milliseconds: 500));
    expect(find.byTooltip('Pause'), findsOneWidget);
    final image =
        await (capture.currentContext!.findRenderObject()
                as RenderRepaintBoundary)
            .toImage(pixelRatio: 2);
    final png = await image.toByteData(format: ui.ImageByteFormat.png);
    image.dispose();
    await File(
      '${await WorkspaceConnection.phoneBase()}/qa-room.png',
    ).writeAsBytes(png!.buffer.asUint8List());
    await tester.pageBack();
    await tester.pumpAndSettle();
    final archives = await RoomArchive.list(room.id);
    expect(archives, isNotEmpty);
    final records = await RoomArchive.read(archives.first).toList();
    expect(records.any((entry) => entry['type'] == 'event'), isTrue);
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();
    await (await RoomArchive.directory(room.id)).delete(recursive: true);
    await Directory(
      '${await WorkspaceConnection.phoneBase()}/${room.id}',
    ).delete(recursive: true);
  });
  testWidgets(
    'SSH from Android verifies host, reads, writes, runs and rejects escape',
    (tester) async {
      const remotePath = String.fromEnvironment('SSH_TEST_PATH');
      const fingerprint = String.fromEnvironment('SSH_TEST_FINGERPRINT');
      expect(
        remotePath,
        isNotEmpty,
        reason: 'Pass the isolated SSH fixture path.',
      );
      expect(fingerprint, isNotEmpty);
      final config = WorkspaceConfig(
        kind: 'ssh',
        host: '127.0.0.1',
        port: 22222,
        username: 'qa',
        password: const String.fromEnvironment(
          'SSH_TEST_PASSWORD',
          defaultValue: 'orgbot-test-only',
        ),
        path: remotePath,
        commands: true,
      );
      // Allow the host runner to restore its ADB reverse after installation.
      await Future<void>.delayed(const Duration(seconds: 3));
      final remote = WorkspaceConnection(config, 'ssh-device-qa');
      var verified = false;
      try {
        await remote.connect(
          trust: (host, type, key) async {
            verified = true;
            expect(key, fingerprint);
            return true;
          },
        );
        expect(verified, isTrue);
        expect(config.fingerprint, fingerprint);
        await remote.execute('workspace_write', {
          'path': 'from-phone.txt',
          'content': 'Android -> SSH -> VPS file',
        });
        expect(
          await remote.execute('workspace_read', {'path': 'from-phone.txt'}),
          'Android -> SSH -> VPS file',
        );
        final result =
            await remote.execute('workspace_run', {
                  'command': 'printf "remote-command-ok"',
                })
                as Map;
        expect(result['exit_code'], 0);
        expect(result['output'], 'remote-command-ok');
        await remote.cancelOperation();
        expect(remote.connected, isFalse);
        expect(
          await remote.execute('workspace_read', {'path': 'from-phone.txt'}),
          'Android -> SSH -> VPS file',
        );
        expect(remote.connected, isTrue);
        await expectLater(
          remote.execute('workspace_read', {'path': 'escape/outside.txt'}),
          throwsStateError,
        );
        await expectLater(
          remote.execute('workspace_write', {
            'path': 'escape/outside.txt',
            'content': 'denied',
          }),
          throwsStateError,
        );
        config.writable = false;
        await expectLater(
          remote.execute('workspace_run', {'command': 'echo denied'}),
          throwsStateError,
        );
        await remote.close();
        await expectLater(
          remote.execute('workspace_read', {'path': 'existing.txt'}),
          throwsStateError,
        );
        final wrong = WorkspaceConnection(
          config.copy()..fingerprint = 'SHA256:wrong',
          'wrong-key',
        );
        await expectLater(wrong.connect(), throwsStateError);
        await wrong.close();
        final keyedConfig = config.copy()
          ..username = 'qa-key'
          ..password = ''
          ..privateKey = utf8.decode(
            base64.decode(
              const String.fromEnvironment('SSH_TEST_PRIVATE_KEY_B64'),
            ),
          )
          ..passphrase = const String.fromEnvironment(
            'SSH_TEST_KEY_PASSPHRASE',
          );
        expect(keyedConfig.privateKey, isNotEmpty);
        final keyed = WorkspaceConnection(keyedConfig, 'ssh-key-device-qa');
        try {
          await keyed.connect();
          expect(
            await keyed.execute('workspace_read', {'path': 'existing.txt'}),
            'original',
          );
        } finally {
          await keyed.close();
        }
      } finally {
        await remote.close();
      }
    },
  );
}
