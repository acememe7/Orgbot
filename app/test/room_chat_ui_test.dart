// The room screen must be a group chat you can talk to, not a log dump.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';

Future<void> pumpRoom(WidgetTester tester) async {
  await tester.binding.setSurfaceSize(const Size(900, 1200));
  addTearDown(() => tester.binding.setSurfaceSize(null));

  final library = Library();
  final bot = BotDef(name: 'Helper', handle: 'helper');
  library.bots.add(bot);
  final room = RoomDef(
    name: 'Release',
    goal: 'ship the notes',
    bots: [RoomSlot(bot.id, 'writer')],
  );

  await tester.pumpWidget(MaterialApp(
    home: RunningRoomPage(room: room, library: library),
  ));
  await tester.pump();
}

void main() {
  testWidgets('the room screen offers a composer to talk to the bots',
      (tester) async {
    await pumpRoom(tester);

    expect(find.text('Release'), findsOneWidget);
    expect(find.text('ship the notes'), findsOneWidget);
    expect(
      find.byType(TextField),
      findsOneWidget,
      reason: 'without an input the room cannot be talked to at all',
    );
    expect(find.byIcon(Icons.arrow_upward), findsOneWidget,
        reason: 'the send control must be present');
  });

  testWidgets('typing a message puts it in the transcript', (tester) async {
    await pumpRoom(tester);

    await tester.enterText(find.byType(TextField), 'how is it going?');
    await tester.tap(find.byIcon(Icons.arrow_upward));
    await tester.pump();

    expect(find.text('how is it going?'), findsOneWidget,
        reason: 'what the person said must appear in the room');
  });

  testWidgets('the board can be shown and hidden', (tester) async {
    await pumpRoom(tester);

    expect(find.byIcon(Icons.checklist_outlined), findsOneWidget);
    await tester.tap(find.byIcon(Icons.checklist_outlined));
    await tester.pump();
    expect(find.byIcon(Icons.checklist), findsOneWidget,
        reason: 'the toggle should flip to the filled icon when open');
  });
}
