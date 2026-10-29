import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/ui/desktop_project_pane.dart';
import 'package:orgbot_app/ui/room_widgets.dart';
import 'package:orgbot_app/workspace/desktop_files.dart';

// A 1x1 transparent PNG.
final _png = Uint8List.fromList(const [
  0x89,
  0x50,
  0x4E,
  0x47,
  0x0D,
  0x0A,
  0x1A,
  0x0A,
  0x00,
  0x00,
  0x00,
  0x0D,
  0x49,
  0x48,
  0x44,
  0x52,
  0x00,
  0x00,
  0x00,
  0x01,
  0x00,
  0x00,
  0x00,
  0x01,
  0x08,
  0x06,
  0x00,
  0x00,
  0x00,
  0x1F,
  0x15,
  0xC4,
  0x89,
  0x00,
  0x00,
  0x00,
  0x0A,
  0x49,
  0x44,
  0x41,
  0x54,
  0x78,
  0x9C,
  0x63,
  0x00,
  0x01,
  0x00,
  0x00,
  0x05,
  0x00,
  0x01,
  0x0D,
  0x0A,
  0x2D,
  0xB4,
  0x00,
  0x00,
  0x00,
  0x00,
  0x49,
  0x45,
  0x4E,
  0x44,
  0xAE,
  0x42,
  0x60,
  0x82,
]);

/// A PNG header with a valid checksum claiming `width` x `height` pixels.
Uint8List _pngClaiming(int width, int height) {
  int crc(List<int> bytes) {
    var c = 0xFFFFFFFF;
    for (final b in bytes) {
      c ^= b;
      for (var k = 0; k < 8; k++) {
        c = (c & 1) != 0 ? 0xEDB88320 ^ (c >>> 1) : c >>> 1;
      }
    }
    return c ^ 0xFFFFFFFF;
  }

  List<int> be(int v) => [v >> 24 & 255, v >> 16 & 255, v >> 8 & 255, v & 255];
  final ihdr = [
    ...'IHDR'.codeUnits,
    ...be(width),
    ...be(height),
    8,
    6,
    0,
    0,
    0,
  ];
  return Uint8List.fromList([
    ..._png.sublist(0, 8),
    ...be(13),
    ...ihdr,
    ...be(crc(ihdr)),
    ..._png.sublist(33),
  ]);
}

void main() {
  group('links resolve only to files in the room project', () {
    const links = RoomFileLinks(
      projectRoot: '/home/aece/projects/Protest',
      open: null,
      child: SizedBox(),
    );

    test('paths a bot writes', () {
      expect(
        links.resolve('/home/aece/projects/Protest/artifacts/shot.png'),
        '/home/aece/projects/Protest/artifacts/shot.png',
      );
      expect(
        links.resolve('artifacts/shot.png'),
        '/home/aece/projects/Protest/artifacts/shot.png',
      );
      expect(
        links.resolve('file:///home/aece/projects/Protest/notes.md'),
        '/home/aece/projects/Protest/notes.md',
      );
    });

    test('anything else', () {
      expect(links.resolve('/home/aece/.ssh/id_ed25519'), isNull);
      expect(links.resolve('../../.ssh/id_ed25519'), isNull);
      expect(
        links.resolve('/home/aece/projects/Protest/../Other/a.png'),
        isNull,
      );
      expect(links.resolve('/home/aece/projects/Protest-old/a.png'), isNull);
      expect(links.resolve('https://example.com/a.png'), isNull);
      expect(links.resolve('#section'), isNull);
      expect(links.resolve('/home/aece/projects/Protest'), isNull);
    });
  });

  testWidgets('a message shows project screenshots and opens linked files', (
    tester,
  ) async {
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(desktopWorkspaceChannel, (call) async {
      return call.method == 'read_preview' ? _png : null;
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(desktopWorkspaceChannel, null),
    );
    late Directory root;
    await tester.runAsync(() async {
      root = await Directory.systemTemp.createTemp('orgbot-links-');
      await Directory('${root.path}/shots').create();
      await File('${root.path}/shots/gas.png').writeAsBytes(_png);
      await File('${root.path}/notes.md').writeAsString('notes');
    });
    addTearDown(() async => root.delete(recursive: true));
    final opened = <String>[];

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: RoomFileLinks(
            projectRoot: root.path,
            open: opened.add,
            child: MessageText(
              text: [
                '[Screenshot](${root.path}/shots/gas.png)',
                '[notes](notes.md)',
                '[elsewhere](/etc/hostname)',
              ].join('\n\n'),
            ),
          ),
        ),
      ),
    );
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pump();
    expect(
      find.byType(ProjectImageThumb),
      findsOneWidget,
      reason: 'screenshots show in the message',
    );

    await tester.tap(find.text('notes', findRichText: true));
    await tester.tap(find.byType(ProjectImageThumb));
    expect(opened, ['${root.path}/notes.md', '${root.path}/shots/gas.png']);

    await tester.tap(find.text('elsewhere', findRichText: true));
    await tester.pump();
    expect(
      opened,
      hasLength(2),
      reason: 'a file outside the project is not opened',
    );
    expect(find.textContaining('outside this room'), findsOneWidget);
  });

  testWidgets('a linked file opens in the project files tab', (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(desktopWorkspaceChannel, (call) async {
      return call.method == 'read_preview'
          ? Uint8List.fromList('the gas notes'.codeUnits)
          : null;
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(desktopWorkspaceChannel, null),
    );
    late Directory root;
    late String file;
    await tester.runAsync(() async {
      root = await Directory(
        (await Directory.systemTemp.createTemp('orgbot-reveal-')).path,
      ).resolveSymbolicLinks().then(Directory.new);
      await Directory(
        '${root.path}/artifacts/curfew-gas',
      ).create(recursive: true);
      file = '${root.path}/artifacts/curfew-gas/verification.md';
      await File(file).writeAsString('the gas notes');
    });
    addTearDown(() async => root.delete(recursive: true));
    final requests = ProjectFileRequests();
    addTearDown(requests.dispose);

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 900,
            height: 620,
            child: DesktopProjectPane(
              projectPath: null,
              initialRootForTesting: root.path,
              requests: requests,
            ),
          ),
        ),
      ),
    );
    await tester.runAsync(() async {
      requests.show(file);
      await Future<void>.delayed(const Duration(milliseconds: 300));
    });
    await tester.pump();
    expect(find.textContaining('the gas notes'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  test('an image claiming enormous dimensions is not decoded', () async {
    await expectLater(
      checkedImage(Future.value(_pngClaiming(1, 1))),
      completes,
    );
    await expectLater(
      checkedImage(Future.value(_pngClaiming(60000, 60000))),
      throwsA(isA<FormatException>()),
    );
  });

  testWidgets('an image a bot generated shows as a picture in its message', (
    tester,
  ) async {
    late Directory root;
    const id = 'att_0123456789abcdef0123456789abcdef';
    await tester.runAsync(() async {
      root = await Directory.systemTemp.createTemp('orgbot-generated-');
      await Directory('${root.path}/$id').create();
      await File('${root.path}/$id/image-0.jpg').writeAsBytes(_png);
    });
    addTearDown(() async => root.delete(recursive: true));
    var opened = 0;
    // The picture is read from disk, which needs the real clock.
    await tester.runAsync(() async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AttachmentTile(
              file: ChatFile(
                id: id,
                name: 'exec-1.png',
                mimeType: 'image/png',
                bytes: _png.length,
                images: 1,
                note: 'Generated by @mira',
              ),
              root: root.path,
              onOpen: () => opened++,
            ),
          ),
        ),
      );
      await Future<void>.delayed(const Duration(milliseconds: 200));
    });
    await tester.pump();
    expect(find.byType(Image), findsOneWidget);
    expect(find.textContaining('Generated by @mira'), findsOneWidget);
    await tester.tap(find.byType(Image));
    expect(opened, 1);
  });
}
