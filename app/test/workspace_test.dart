import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:orgbot_app/workspace/workspace_screen.dart';
import 'package:orgbot_app/workspace/workspace.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/room_file.dart';
import 'package:orgbot_app/net/room_client.dart';
import 'package:orgbot_app/state/room_archive.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('command durations are bounded and default to five minutes', () {
    expect(commandTimeoutSeconds({}), 300);
    expect(commandTimeoutSeconds({'timeout_seconds': 1800}), 1800);
    for (final value in [0, -1, 1801, 2.5, '60', true]) {
      expect(
        () => commandTimeoutSeconds({'timeout_seconds': value}),
        throwsFormatException,
      );
    }
  });
  testWidgets(
    'private key paste and import preserve PEM newlines without displaying the key',
    (tester) async {
      const pem =
          '-----BEGIN OPENSSH PRIVATE KEY-----\nfixture-key-data\n-----END OPENSSH PRIVATE KEY-----';
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async =>
            call.method == 'Clipboard.getData' ? {'text': pem} : null,
      );
      messenger.setMockMethodCallHandler(
        workspaceChannel,
        (call) async => {'content': '$pem\n', 'name': 'key'},
      );
      addTearDown(() {
        messenger.setMockMethodCallHandler(SystemChannels.platform, null);
        messenger.setMockMethodCallHandler(workspaceChannel, null);
      });
      await tester.pumpWidget(
        MaterialApp(
          home: WorkspaceScreen(
            config: WorkspaceConfig(kind: 'ssh', host: 'example.test'),
            roomId: 'paste-test',
          ),
        ),
      );
      await tester.tap(find.text('Private key'));
      await tester.pumpAndSettle();
      final field = find.byWidgetPredicate(
        (w) =>
            w is TextField &&
            w.decoration?.labelText == 'OpenSSH / PEM private key',
      );
      await tester.ensureVisible(find.text('Paste key'));
      await tester.tap(find.text('Paste key'));
      await tester.pumpAndSettle();
      await tester.scrollUntilVisible(
        field,
        -180,
        scrollable: find.byType(Scrollable).first,
      );
      var widget = tester.widget<TextField>(field);
      expect(widget.controller!.text, pem);
      expect(widget.obscureText, isTrue);
      expect(widget.readOnly, isTrue);
      widget.controller!.clear();
      await tester.ensureVisible(find.text('Import key file'));
      await tester.tap(find.text('Import key file'));
      await tester.pumpAndSettle();
      await tester.scrollUntilVisible(
        field,
        -180,
        scrollable: find.byType(Scrollable).first,
      );
      widget = tester.widget<TextField>(field);
      expect(widget.controller!.text, pem);
    },
  );
  testWidgets(
    'folder selection loads contents and saves the chosen absolute path',
    (tester) async {
      final dir = await tester.runAsync(
        () => Directory.systemTemp.createTemp('orgbot-folder-picker-'),
      );
      final nested = Directory('${dir!.path}/selected');
      await tester.runAsync(() async {
        await nested.create();
        await File('${nested.path}/notes.txt').writeAsString('Saved workspace');
      });
      WorkspaceConfig? selected;
      Future<void> loaded(String name) async {
        for (var i = 0; i < 50 && find.text(name).evaluate().isEmpty; i++) {
          await tester.runAsync(
            () => Future<void>.delayed(const Duration(milliseconds: 40)),
          );
          await tester.pump();
        }
        expect(find.text(name), findsOneWidget);
      }

      try {
        await tester.pumpWidget(
          MaterialApp(
            home: Builder(
              builder: (context) => TextButton(
                onPressed: () async {
                  selected = await Navigator.push<WorkspaceConfig>(
                    context,
                    MaterialPageRoute(
                      builder: (_) => WorkspaceFilesScreen(
                        config: WorkspaceConfig(
                          path: dir.path,
                          writable: false,
                        ),
                        roomId: 'folder-picker',
                        selectFolder: true,
                      ),
                    ),
                  );
                },
                child: const Text('Choose'),
              ),
            ),
          ),
        );
        await tester.tap(find.text('Choose'));
        await tester.pump();
        await loaded('selected');
        await tester.pumpAndSettle();
        await tester.tap(find.text('selected'));
        await tester.pump();
        await loaded('notes.txt');
        await tester.tap(find.text('Use this folder'));
        await tester.pumpAndSettle();
        expect(
          selected?.path,
          await tester.runAsync(nested.resolveSymbolicLinks),
        );
        final library = Library(saver: MemorySaver());
        await library.load();
        library.addRoom(RoomDef(name: 'Selected folder', workspace: selected));
        await library.save();
        final reload = Library(saver: library.saver);
        await reload.load();
        expect(reload.rooms.single.workspace.path, selected!.path);
      } finally {
        await tester.pumpWidget(const SizedBox.shrink());
        await tester.runAsync(() => dir.delete(recursive: true));
      }
    },
  );
  test(
    'run archives survive reload without retaining executable workspace requests',
    () async {
      final id = 'archive_test_${DateTime.now().microsecondsSinceEpoch}';
      final errors = <String>[];
      final archive = await RoomArchive.open(id, errors.add);
      addTearDown(
        () async => (await RoomArchive.directory(id)).delete(recursive: true),
      );
      archive.append([
        {
          'type': 'event',
          'envelope': {'seq': 1},
        },
        {
          'type': 'workspace_call',
          'arguments': {'command': 'must not replay'},
        },
      ]);
      await archive.close();
      await archive.file.writeAsString('{interrupted', mode: FileMode.append);
      final recovered = await RoomArchive.read(
        (await RoomArchive.list(id)).single,
      ).toList();
      expect(recovered.length, 1);
      expect(recovered.single['type'], 'event');
      expect(errors, isEmpty);
    },
  );
  test(
    'phone files are bounded, atomic and confined; read-only blocks mutation',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'orgbot-files-test',
      );
      final outside = await Directory.systemTemp.createTemp(
        'orgbot-outside-test',
      );
      addTearDown(() => directory.delete(recursive: true));
      addTearDown(() => outside.delete(recursive: true));
      await File('${outside.path}/secret.txt').writeAsString('outside');
      final config = WorkspaceConfig(path: directory.path);
      final workspace = WorkspaceConnection(config, 'test');
      await workspace.connect();
      addTearDown(workspace.close);
      await workspace.execute('workspace_write', {
        'path': 'note.txt',
        'content': 'hello λ',
      });
      expect(
        await workspace.execute('workspace_read', {'path': 'note.txt'}),
        'hello λ',
      );
      expect(
        (await File('${directory.path}/note.txt').stat()).mode & 0x1ff,
        0x180,
      );
      expect(
        (await Process.run('chmod', [
          '700',
          '${directory.path}/note.txt',
        ])).exitCode,
        0,
      );
      await workspace.execute('workspace_write', {
        'path': 'note.txt',
        'content': 'replacement',
      });
      expect(
        await File('${directory.path}/note.txt').readAsString(),
        'replacement',
      );
      expect(
        (await File('${directory.path}/note.txt').stat()).mode & 0x1ff,
        0x1c0,
      );
      await Link('${directory.path}/escape').create(outside.path);
      await expectLater(
        workspace.execute('workspace_read', {'path': 'escape/secret.txt'}),
        throwsStateError,
      );
      await expectLater(
        workspace.execute('workspace_write', {
          'path': 'escape/secret.txt',
          'content': 'oops',
        }),
        throwsStateError,
      );
      await expectLater(
        workspace.execute('workspace_read', {'path': '../secret.txt'}),
        throwsFormatException,
      );
      await expectLater(
        workspace.execute('workspace_write', {
          'path': 'note.txt',
          'content': 'x' * (maxFileBytes + 1),
        }),
        throwsStateError,
      );
      expect(
        await File('${directory.path}/note.txt').readAsString(),
        'replacement',
      );
      config.writable = false;
      await expectLater(
        workspace.execute('workspace_write', {
          'path': 'note.txt',
          'content': 'oops',
        }),
        throwsStateError,
      );
      await expectLater(
        workspace.execute('workspace_run', {'command': 'echo no'}),
        throwsStateError,
      );
      expect(
        await File('${outside.path}/secret.txt').readAsString(),
        'outside',
      );
    },
  );
  test(
    'explicit workspace descriptions reach phone and desktop engines',
    () {
      final bot = BotDef(name: 'Demo', provider: ProviderKind.scripted);
      final room = RoomDef(
        name: 'test',
        bots: [RoomSlot(bot.id)],
        workspace: WorkspaceConfig(
          kind: 'ssh',
          host: 'example.test',
          password: 'never-in-model',
        ),
      );
      final description = {
        'label': 'SSH example',
        'root': '/work',
        'writable': false,
        'commands': false,
      };
      final phone = buildRoomFile(
        room,
        [bot],
        canRunClis: false,
        workspace: description,
      );
      expect(phone.toml, contains('[workspace]'));
      expect(phone.toml, isNot(contains('never-in-model')));
      final desktop = buildRoomFile(
        room,
        [bot],
        canRunClis: true,
        workspace: description,
      );
      expect(desktop.toml, contains('[workspace]'));
      expect(desktop.toml, contains('root = "/work"'));
      expect(desktop.toml, contains('commands = false'));
      expect(RoomDef.fromJson(room.toJson()).workspace.host, 'example.test');
    },
  );
  test('local controls reach the native transport callback', () async {
    final controls = <Map<String, Object?>>[];
    final client = LocalRoomClient(onSend: controls.add);
    client.pause();
    client.resume();
    client.answerBot('task_1', 'answer');
    client.resync();
    expect(controls.map((c) => c['type']), [
      'pause',
      'resume',
      'answer_bot',
      'resync',
    ]);
    expect(client.sent, isEmpty);
    await client.close();
  });
}
