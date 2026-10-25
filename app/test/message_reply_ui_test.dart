import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/ui/room_widgets.dart';
import 'room_chat_ui_test.dart' show FakeClient, seeded, event;

void main() {
  testWidgets(
    'reply picks the message, shows its context and supports keyboard send',
    (tester) async {
      final client = FakeClient();
      final store = seeded(client);
      final scroll = ScrollController();
      final input = TextEditingController();
      final focus = FocusNode();
      addTearDown(store.dispose);
      addTearDown(scroll.dispose);
      addTearDown(input.dispose);
      addTearDown(focus.dispose);
      client.emit(
        event({
          'event': 'message_posted',
          'message': {
            'id': 'review',
            'room': 'r',
            'author': {'kind': 'agent', 'agent': 'bot_b'},
            'body': 'The file permissions need another look.',
            'intent': 'say',
            'sent_at': 0,
          },
        }),
      );
      ChatMessage? reply;
      String? sentReply;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: StatefulBuilder(
              builder: (context, update) => Column(
                children: [
                  Expanded(
                    child: Transcript(
                      store: store,
                      controller: scroll,
                      onReply: (message) {
                        update(() => reply = message);
                        focus.requestFocus();
                      },
                    ),
                  ),
                  Composer(
                    controller: input,
                    focusNode: focus,
                    replyTo: reply,
                    replyAuthor: 'Quill',
                    onCancelReply: () => update(() => reply = null),
                    onSend: () => sentReply = reply?.id,
                  ),
                ],
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('Reply to quill'));
      await tester.pump();
      expect(find.text('Reply to Quill'), findsOneWidget);
      expect(focus.hasFocus, isTrue);
      await tester.enterText(
        find.byType(TextField),
        'Check the import path too.',
      );
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      expect(sentReply, 'review');
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(find.text('Reply to Quill'), findsNothing);
      expect(input.text, 'Check the import path too.');
      expect(tester.takeException(), isNull);
    },
  );
}
