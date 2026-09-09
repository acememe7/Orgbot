import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/workspace/attachments.dart';
import 'package:orgbot_app/workspace/workspace.dart';
import 'package:orgbot_app/ui/room_widgets.dart';
import 'room_chat_ui_test.dart' show FakeClient, seeded, event;

const file = ChatFile(
  id: 'att_0123456789abcdef0123456789abcdef',
  name: 'A research document with a long name.pdf',
  mimeType: 'application/pdf',
  bytes: 3210,
  hasText: true,
  note: 'Document text included. Original saved.',
);
void main() {
  testWidgets(
    'phone composer attaches, removes and blocks send during preparation without overflow',
    (tester) async {
      tester.view.physicalSize = const Size(320, 700);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final input = TextEditingController();
      addTearDown(input.dispose);
      var picked = 0, removed = 0, sent = 0;
      Future<void> show(bool busy) => tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.bottomCenter,
              child: Composer(
                controller: input,
                onSend: () => sent++,
                onAttach: () => picked++,
                onRemoveAttachment: (_) => removed++,
                attachments: const [file],
                mentions: const ['iris'],
                busy: busy,
              ),
            ),
          ),
        ),
      );
      await show(false);
      await tester.tap(find.byTooltip('Add to message'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Attach files'));
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('Remove ${file.name}'));
      await tester.tap(find.byType(FilledButton));
      expect([picked, removed, sent], [1, 1, 1]);
      expect(tester.takeException(), isNull);
      await show(true);
      expect(
        tester.widget<FilledButton>(find.byType(FilledButton)).onPressed,
        isNull,
      );
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'confirmed attachments retain metadata and only clear the matching pending message',
    (tester) async {
      final client = FakeClient();
      final store = seeded(client);
      addTearDown(store.dispose);
      store.addPending('Alex', 'Review this');
      store.addPending('Alex', 'Review this', files: const [file]);
      client.emit(
        event({
          'event': 'message_posted',
          'message': {
            'id': 'file-msg',
            'author': {'kind': 'human', 'name': 'Alex'},
            'body': 'Review this',
            'intent': 'ask',
            'sent_at': 0,
            'files': [file.toJson()],
          },
        }, seq: 10),
      );
      expect(store.messages.where(store.isPending).length, 1);
      expect(store.messages.last.files, isEmpty);
      final confirmed = store.messages.firstWhere((m) => m.id == 'file-msg');
      expect(confirmed.files.single.name, file.name);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 230,
              child: AttachmentTile(file: confirmed.files.single),
            ),
          ),
        ),
      );
      expect(find.text(file.name), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );
  test(
    'saved drafts reopen and originals import unchanged with write permissions enforced',
    () async {
      final room = RoomAttachments(
        'room_attachment_test_${DateTime.now().microsecondsSinceEpoch}',
      );
      final dir = await room.directory;
      final folder = await Directory(
        '${dir.path}/${file.id}',
      ).create(recursive: true);
      final original = await File(
        '${folder.path}/original',
      ).writeAsBytes([0, 255, 128, 10, 2]);
      final workspaceDir = await Directory.systemTemp.createTemp(
        'orgbot-import-',
      );
      final connection = WorkspaceConnection(
        WorkspaceConfig(path: workspaceDir.path),
        'attachment_test',
      );
      try {
        await room.saveDraft([file]);
        expect(
          (await RoomAttachments(room.roomId).loadDraft()).single.toJson(),
          file.toJson(),
        );
        await expectLater(room.original('../outside'), throwsFormatException);
        await connection.connect();
        final result =
            await connection.importAttachment(original, '../../research.bin')
                as Map;
        final name = result['written'] as String;
        expect(name.contains('/'), isFalse);
        expect(await File('${workspaceDir.path}/$name').readAsBytes(), [
          0,
          255,
          128,
          10,
          2,
        ]);
        connection.config.writable = false;
        await expectLater(
          connection.importAttachment(original, 'again'),
          throwsStateError,
        );
        await room.saveDraft([]);
        expect(await room.loadDraft(), isEmpty);
        expect(await original.exists(), isTrue);
        expect(jsonEncode(file.toJson()), isNot(contains('base64')));
      } finally {
        await connection.close();
        await dir.parent.delete(recursive: true);
        await workspaceDir.delete(recursive: true);
      }
    },
  );
}
