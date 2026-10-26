import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/state/room_store.dart' show toolLabel;
import 'package:orgbot_app/ui/room_widgets.dart';
import 'package:orgbot_app/ui/widgets/mentions.dart';

void main() {
  group('MentionQuery', () {
    TextEditingValue at(String text) => TextEditingValue(
      text: text,
      selection: TextSelection.collapsed(offset: text.length),
    );

    test('finds the tag being typed at the cursor', () {
      expect(MentionQuery.at(at('@'))?.text, '');
      expect(MentionQuery.at(at('hey @gr'))?.text, 'gr');
      expect(MentionQuery.at(at('hey @gr'))?.start, 4);
      expect(MentionQuery.at(at('mail bob@example')), isNull);
      expect(MentionQuery.at(at('@grok done')), isNull);
    });

    test('completes the tag and leaves the cursor after a space', () {
      final value = at('hi @gr');
      final done = MentionQuery.at(value)!.complete(value, 'grok');
      expect(done.text, 'hi @grok ');
      expect(done.selection.baseOffset, done.text.length);
    });

    test('everyone answers to all, everyone and everybody', () {
      expect(MentionOption.all.matches('a'), isTrue);
      expect(MentionOption.all.matches('every'), isTrue);
      expect(MentionOption.all.matches('x'), isFalse);
      const bot = MentionOption(handle: 'grok', name: 'Miles');
      expect(bot.matches('mi'), isTrue);
      expect(bot.matches('gr'), isTrue);
      expect(bot.matches('op'), isFalse);
    });
  });

  test('tool calls read as plain names', () {
    expect(toolLabel('mcp__orgbot_room__list_tasks'), 'room · list tasks');
    expect(toolLabel('mcp__github__create_issue'), 'github · create issue');
    expect(toolLabel('ToolSearch'), 'ToolSearch');
  });

  testWidgets('typing @ offers everyone first and Enter inserts the tag', (
    tester,
  ) async {
    final input = TextEditingController();
    final focus = FocusNode();
    var sent = 0;
    addTearDown(input.dispose);
    addTearDown(focus.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.bottomCenter,
            child: Composer(
              desktop: true,
              controller: input,
              focusNode: focus,
              onSend: () => sent++,
              mentionOptions: const [
                MentionOption(handle: 'grok', name: 'Miles'),
                MentionOption(handle: 'opus', name: 'Julian'),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.byType(TextField));
    await tester.enterText(find.byType(TextField), '@');
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('mention-suggestions')), findsOneWidget);
    expect(find.text('@all'), findsOneWidget);
    expect(find.text('@grok'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(input.text, '@all ');
    expect(sent, 0, reason: 'accepting a suggestion must not send');
    expect(find.byKey(const ValueKey('mention-suggestions')), findsNothing);

    await tester.enterText(find.byType(TextField), '@all hi @ju');
    await tester.pumpAndSettle();
    expect(find.text('@opus'), findsOneWidget);
    expect(find.text('@grok'), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pumpAndSettle();
    expect(input.text, '@all hi @opus ');

    await tester.enterText(find.byType(TextField), 'x @');
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('mention-suggestions')), findsNothing);
  });

  testWidgets('tags in a message are drawn as one paragraph', (tester) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(body: MessageText(text: '@all please say hi to bob@x.io')),
      ),
    );
    expect(
      find.text('@all please say hi to bob@x.io', findRichText: true),
      findsOneWidget,
    );
  });
}
