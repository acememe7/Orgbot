// Run only as com.orgbot.app.qa. Uses private generated fixtures, never owner files.
import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:orgbot_app/main.dart' show orgBotTheme;
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/protocol/models.dart' show ChatFile;
import 'package:orgbot_app/state/room_archive.dart';
import 'package:orgbot_app/ui/engine_room_screen.dart';
import 'package:orgbot_app/workspace/attachments.dart';
import 'package:orgbot_app/workspace/workspace.dart';
import 'attachment_fixtures.dart';

void check(bool ok, String message) {
  if (!ok) throw StateError(message);
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final library = Library(saver: MemorySaver());
  await library.load();
  final bot = library.addBot(
    BotDef(name: 'Iris', handle: 'iris', provider: ProviderKind.scripted),
  );
  final room = library.addRoom(
    RoomDef(
      id: 'room_attachments_qa',
      name: 'Attachment check',
      goal: 'Read the attached research.',
      bots: [RoomSlot(bot.id)],
    ),
  );
  final base = await WorkspaceConnection.phoneBase();
  await Directory(base).create(recursive: true);
  final result = File('$base/attachments-result.json');
  final passed = <String>[];
  Future<void> report(String stage) => result.writeAsString(
    jsonEncode({'passed': passed, 'stage': stage}),
    flush: true,
  );
  final attachments = RoomAttachments(room.id);
  final files = <ChatFile>[];
  WorkspaceConnection? local, remote;
  Future<void> waitFor(bool Function() done, String label) async {
    for (var i = 0; i < 150 && !done(); i++) {
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
    check(done(), label);
  }

  runApp(
    MaterialApp(
      theme: orgBotTheme(Brightness.dark),
      home: const Scaffold(body: Center(child: Text('Checking attachments…'))),
    ),
  );
  try {
    for (final entry in attachmentFixtures.entries) {
      await report('prepare ${entry.key}');
      final id = 'att_${files.length.toRadixString(16).padLeft(32, '0')}';
      final folder = await Directory(
        '${(await attachments.directory).path}/$id',
      ).create(recursive: true);
      final bytes = base64Decode(entry.value);
      await File('${folder.path}/original').writeAsBytes(bytes);
      await File('${folder.path}/manifest.json').writeAsString(
        jsonEncode({
          'id': id,
          'name': entry.key,
          'mime_type': entry.key.endsWith('.pdf')
              ? 'application/pdf'
              : 'application/octet-stream',
          'bytes': bytes.length,
        }),
      );
      final value = await attachmentChannel.invokeMapMethod<String, Object?>(
        'prepare',
        {'room': room.id, 'id': id},
      );
      final file = ChatFile.fromJson(value!);
      if (entry.key.endsWith('.png')) {
        check(
          file.images == 1 &&
              await File('${folder.path}/image-0.jpg').length() > 0,
          'Image preview missing',
        );
      } else if (entry.key.endsWith('.bin')) {
        check(
          !file.hasText && file.note.contains('No automatic'),
          'Binary must be explicit',
        );
      } else {
        check(
          file.hasText &&
              (await File(
                '${folder.path}/context.txt',
              ).readAsString()).contains('73'),
          'Document text missing: ${entry.key}',
        );
        if (entry.key.endsWith('.xlsx')) {
          check(
            (await File(
              '${folder.path}/context.txt',
            ).readAsString()).contains('A1=Participants'),
            'Shared spreadsheet strings missing',
          );
        }
      }
      check(
        listEquals(await (await attachments.original(id)).readAsBytes(), bytes),
        'Original modified',
      );
      files.add(file);
      passed.add('native ${entry.key}');
    }
    await attachments.saveDraft(files.take(2).toList());
    check(
      (await RoomAttachments(room.id).loadDraft()).length == 2,
      'Draft not saved',
    );
    passed.add('attachment draft survives reload');
    await report('phone and SSH import');
    final folder = await Directory(
      '$base/attachment-work',
    ).create(recursive: true);
    local = WorkspaceConnection(WorkspaceConfig(path: folder.path), room.id);
    await local.connect();
    final source = await attachments.original(files.last.id);
    final imported =
        await local.importAttachment(source, files.last.name) as Map;
    check(
      listEquals(
        await File('${folder.path}/${imported['written']}').readAsBytes(),
        await source.readAsBytes(),
      ),
      'Phone copy changed bytes',
    );
    remote = WorkspaceConnection(
      WorkspaceConfig(
        kind: 'ssh',
        host: '127.0.0.1',
        port: 22222,
        username: 'qa',
        password: const String.fromEnvironment('SSH_TEST_PASSWORD'),
        path: const String.fromEnvironment('SSH_TEST_PATH'),
        fingerprint: const String.fromEnvironment('SSH_TEST_FINGERPRINT'),
        commands: true,
      ),
      room.id,
    );
    await remote.connect();
    final copy = await remote.importAttachment(source, files.last.name) as Map;
    final output =
        await remote.execute('workspace_run', {
              'command': 'base64 ${copy['written']}',
            })
            as Map;
    check(
      (output['output'] as String).trim() ==
          base64Encode(await source.readAsBytes()),
      'SSH copy changed bytes',
    );
    passed.add('phone and SSH originals identical');
    await report('native room context and restart');
    final controller = library.session(room);
    await controller.start();
    check(
      await controller.say(
        'Alex',
        'Review the research files.',
        files: files.take(5).toList(),
      ),
      'Message rejected',
    );
    await waitFor(
      () => controller.store!.messages.any(
        (m) => m.files.length == 5 && !controller.store!.isPending(m),
      ),
      'Attachments not committed',
    );
    await waitFor(
      () => controller.store!.messages.any((m) => !m.author.isHuman),
      'Bot did not reply',
    );
    await library.stopRoom(room.id);
    final state = await (await RoomArchive.stateFile(room.id)).readAsString();
    check(
      state.contains('73 participants') &&
          state.contains('image-0.jpg') &&
          !state.contains('base64'),
      'Native bot context missing or bloated',
    );
    final restored = library.session(room);
    await restored.start();
    await waitFor(
      () => restored.store!.messages.any((m) => m.files.length == 5),
      'Files not restored',
    );
    check(restored.store!.isPaused, 'Restart unexpectedly resumed bots');
    passed.add('native bot receives content; chat files survive restart');
    await attachments.saveDraft([]);
    await result.writeAsString(
      jsonEncode({'ok': true, 'passed': passed}),
      flush: true,
    );
    runApp(
      MaterialApp(
        debugShowCheckedModeBanner: false,
        theme: orgBotTheme(Brightness.dark),
        home: EngineRoomScreen(room: room, library: library),
      ),
    );
  } catch (e) {
    await result.writeAsString(
      jsonEncode({'ok': false, 'passed': passed, 'error': e.toString()}),
      flush: true,
    );
  } finally {
    await local?.close();
    await remote?.close();
  }
}
