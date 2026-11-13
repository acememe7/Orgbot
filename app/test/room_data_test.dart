// Deleting a room deletes its saved data, never the project.
//
// From 13 November 2026: deleted rooms kept 165 GB of history and working
// copies, which the owner had to remove by hand.
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/state/room_data.dart';

void main() {
  late Directory root;

  setUp(() async {
    root = await Directory.systemTemp.createTemp('orgbot-room-data-');
  });

  tearDown(() async {
    if (await root.exists()) {
      await Process.run('chmod', ['-R', 'u+rwX', root.path]);
      await root.delete(recursive: true);
    }
  });

  test('a room\'s folders go, read-only copies included, and links are not followed', () async {
    final project = Directory('${root.path}/project')..createSync();
    final kept = File('${project.path}/main.dart')..writeAsStringSync('keep me');
    final history = Directory('${root.path}/data/.history/room_1')..createSync(recursive: true);
    final service = Directory('${root.path}/data/.services/room_1')..createSync(recursive: true);
    File('${service.path}/room.toml').writeAsStringSync('x');
    final copy = Directory('${history.path}/workspaces/agents/p/bot/project/assets')
      ..createSync(recursive: true);
    File('${copy.path}/logo.png').writeAsStringSync('png');
    Link('${history.path}/workspaces/agents/p/bot/project/link').createSync(project.path);
    await Process.run('chmod', ['0555', copy.path]);

    await deleteFolders(Future.value([history.path, service.path]), keep: [project.path]);

    expect(history.existsSync(), isFalse);
    expect(service.existsSync(), isFalse);
    expect(kept.readAsStringSync(), 'keep me', reason: 'a link inside is not followed');
  });

  test('nothing is deleted when the project is inside the room\'s data', () async {
    final history = Directory('${root.path}/.history/room_2')..createSync(recursive: true);
    final project = Directory('${history.path}/my-project')..createSync();
    await expectLater(
      deleteFolders(Future.value([history.path]), keep: [project.path]),
      throwsA(isA<FileSystemException>()),
    );
    expect(project.existsSync(), isTrue);
  });

  test('a link in place of a room folder is refused, and missing folders are fine', () async {
    final elsewhere = Directory('${root.path}/elsewhere')..createSync();
    File('${elsewhere.path}/file').writeAsStringSync('x');
    final link = Link('${root.path}/.history/room_3')..createSync(elsewhere.path, recursive: true);
    await expectLater(
      deleteFolders(Future.value([link.path])),
      throwsA(isA<FileSystemException>()),
    );
    expect(File('${elsewhere.path}/file').existsSync(), isTrue);
    await deleteFolders(Future.value(['${root.path}/.history/room_missing']));
  });
}
