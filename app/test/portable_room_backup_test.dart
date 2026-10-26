import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/backup.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/workspace/workspace.dart';

void main() {
  final room = RoomDef(
    id: 'room_original',
    name: 'Design review',
    goal: 'Review the project',
    path: '/private/project',
    memory: 'Decision record',
    workspace: WorkspaceConfig(
      kind: 'ssh',
      host: 'private.example',
      username: 'owner',
      password: 'secret',
      privateKey: 'PRIVATE KEY',
    ),
    bots: [RoomSlot('bot_original')],
  );
  final bot = BotDef(
    id: 'bot_original',
    name: 'Reviewer',
    provider: ProviderKind.deepseek,
    apiKey: 'secret-key',
  );

  test(
    'round trips safe definitions and restores to a new paused room',
    () async {
      final bytes = await PortableRoomBackup.export(
        room: room,
        bots: [bot],
        state: {
          'version': 1,
          'engine': {
            'room': {'id': room.id, 'phase': 'paused'},
            'log': [],
          },
        },
        files: {'project/README.md': utf8.encode('Project notes')},
        isStopped: true,
      );
      final backup = PortableRoomBackup.parse(bytes);
      expect(backup.bots.single.apiKey, isEmpty);
      expect(backup.room.workspace.password, isEmpty);
      expect(backup.room.workspace.privateKey, isEmpty);
      expect(backup.room.workspace.kind, 'phone');
      expect(backup.state['version'], 1);

      final restored = backup.restoreDefinitions(
        newRoomId: 'room_restored',
        newProjectPath: '/new/project',
      );
      expect(restored.room.id, 'room_restored');
      expect(restored.room.path, '/new/project');
      expect(restored.room.bots.single.botId, restored.bots.single.id);
      expect(restored.room.workspace.commands, isFalse);
      expect(restored.bots.single.apiKey, isEmpty);
      // A bundle is data only; parsing and definition creation cannot open a session.
      expect(restored.room.id, isNot(room.id));
      final library = Library(saver: MemorySaver());
      await library.load();
      library.bots.addAll(restored.bots);
      library.rooms.add(restored.room);
      await library.save();
      expect(library.sessions, isEmpty);
      library.dispose();
    },
  );

  test('rejects active source, hash corruption, and traversal paths', () async {
    await expectLater(
      PortableRoomBackup.export(
        room: room,
        bots: [bot],
        state: {},
        files: {},
        isStopped: false,
      ),
      throwsStateError,
    );
    await expectLater(
      PortableRoomBackup.export(
        room: room,
        bots: [bot],
        state: {
          'version': 1,
          'engine': {
            'room': {'id': room.id, 'phase': 'running'},
            'log': [],
          },
        },
        files: {},
        isStopped: true,
      ),
      throwsStateError,
    );
    final bytes = await PortableRoomBackup.export(
      room: room,
      bots: [bot],
      state: {
        'version': 1,
        'engine': {
          'room': {'id': room.id, 'phase': 'paused'},
          'log': [],
        },
      },
      files: {
        'project/readme.md': [65],
      },
      isStopped: true,
    );
    final value = jsonDecode(utf8.decode(bytes)) as Map<String, Object?>;
    (value['files'] as Map)['project/readme.md']['data'] = 'Qg==';
    expect(
      () => PortableRoomBackup.parse(utf8.encode(jsonEncode(value))),
      throwsFormatException,
    );
    final traversal = jsonDecode(utf8.decode(bytes)) as Map<String, Object?>;
    (traversal['files'] as Map).remove('project/readme.md');
    (traversal['files'] as Map)['project/../../outside'] = {
      'sha256': sha256.convert([65]).toString(),
      'data': 'QQ==',
    };
    traversal.remove('manifest_sha256');
    traversal['manifest_sha256'] = sha256
        .convert(utf8.encode(jsonEncode(traversal)))
        .toString();
    expect(
      () => PortableRoomBackup.parse(utf8.encode(jsonEncode(traversal))),
      throwsFormatException,
    );
  });

  test('writes project files only to a new folder', () async {
    final temp = await Directory.systemTemp.createTemp('orgbot-backup-test-');
    addTearDown(() => temp.delete(recursive: true));
    final backup = PortableRoomBackup.parse(
      await PortableRoomBackup.export(
        room: room,
        bots: [bot],
        state: {
          'version': 1,
          'engine': {
            'room': {'id': room.id, 'phase': 'paused'},
            'log': [],
          },
        },
        files: {'project/docs/brief.txt': utf8.encode('brief')},
        isStopped: true,
      ),
    );
    final target = Directory('${temp.path}/restored');
    await backup.restoreFiles(target);
    expect(await File('${target.path}/docs/brief.txt').readAsString(), 'brief');
    if (Platform.isLinux) {
      expect(
        (await File('${target.path}/docs/brief.txt').stat()).mode & 0x1ff,
        0x180,
      );
      expect((await target.stat()).mode & 0x1ff, 0x1c0);
    }
    await expectLater(
      backup.restoreFiles(target),
      throwsA(isA<FileSystemException>()),
    );
    expect(await File('${target.path}/docs/brief.txt').readAsString(), 'brief');
  });

  test(
    'preserves executable status without restoring shared or special bits',
    () async {
      if (!Platform.isLinux) return;
      final root = await Directory.systemTemp.createTemp(
        'orgbot-project-modes-',
      );
      final executable = await File(
        '${root.path}/bin/run.sh',
      ).create(recursive: true);
      await executable.writeAsString('#!/bin/sh\necho safe\n');
      final data = await File('${root.path}/notes.txt').writeAsString('notes');
      for (final change in [
        await Process.run('chmod', ['4751', executable.path]),
        await Process.run('chmod', ['0644', data.path]),
      ]) {
        expect(change.exitCode, 0);
      }
      final modes = <String, int>{};
      final files = await PortableRoomBackup.readProjectFiles(
        root,
        readFile: (_, path) => File(path).readAsBytes(),
        onFileMode: (path, mode) => modes[path] = mode,
      );
      final bytes = await PortableRoomBackup.export(
        room: room,
        bots: [bot],
        state: {},
        files: files,
        projectFileModes: modes,
        isStopped: true,
      );
      final manifest = jsonDecode(utf8.decode(bytes)) as Map<String, Object?>;
      expect(
        ((manifest['files'] as Map)['project/bin/run.sh'] as Map)['mode'],
        0x49,
      );
      final backup = PortableRoomBackup.parse(bytes);
      final target = Directory('${root.path}-restored');
      addTearDown(() async {
        await root.delete(recursive: true);
        if (await target.exists()) await target.delete(recursive: true);
      });
      await backup.restoreFiles(target);

      expect(
        (await File('${target.path}/bin/run.sh').stat()).mode & 0x1ff,
        0x1c0,
      );
      expect(
        (await File('${target.path}/notes.txt').stat()).mode & 0x1ff,
        0x180,
      );
    },
  );

  test('omits common credential and git configuration files', () async {
    final root = await Directory.systemTemp.createTemp('orgbot-project-test-');
    addTearDown(() => root.delete(recursive: true));
    await File('${root.path}/README.md').writeAsString('notes');
    await File('${root.path}/.env').writeAsString('TOKEN=secret');
    await File('${root.path}/id_rsa').writeAsString('private key');
    await Directory('${root.path}/.git').create();
    await File('${root.path}/.git/config').writeAsString('credential.helper');
    final files = await PortableRoomBackup.readProjectFiles(
      root,
      readFile: (_, path) => File(path).readAsBytes(),
    );
    expect(files.keys, ['project/README.md']);
  });

  test('rejects oversized bundle input before decoding', () {
    expect(
      () => PortableRoomBackup.parse(
        List<int>.filled(PortableRoomBackup.maxBundleBytes + 1, 0),
      ),
      throwsFormatException,
    );
  });

  test('rejects symbolic links in selected project folders', () async {
    final root = await Directory.systemTemp.createTemp('orgbot-project-link-');
    final external = await File(
      '${root.path}/outside.txt',
    ).writeAsString('outside');
    await Link('${root.path}/linked.txt').create(external.path);
    addTearDown(() => root.delete(recursive: true));
    await expectLater(
      PortableRoomBackup.readProjectFiles(
        root,
        readFile: (_, path) => File(path).readAsBytes(),
      ),
      throwsFormatException,
    );
  });
}
