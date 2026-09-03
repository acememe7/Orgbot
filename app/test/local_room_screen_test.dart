// The on-device room screen: with the default scripted bots (no API keys)
// a whole room runs and completes inside the widget, showing board and chat.
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:gptbots_app/ui/local_room_screen.dart';

void main() {
  testWidgets('a local room runs to completion from the screen', (tester) async {
    await tester.pumpWidget(const MaterialApp(home: LocalRoomScreen()));

    // Defaults: scripted demo bots for lead/writer/qa - no keys needed.
    final scrollable = find.byType(Scrollable).first;
    await tester.scrollUntilVisible(find.text('Run room'), 300,
        scrollable: scrollable);
    await tester.tap(find.text('Run room'));
    await tester.pump();

    // Let the room run on the fake clock (writer works ~200ms, then review,
    // then submit) - pump well past completion.
    for (var i = 0; i < 12; i++) {
      await tester.pump(const Duration(milliseconds: 250));
    }
    await tester.pump(const Duration(milliseconds: 100));

    await tester.scrollUntilVisible(
        find.textContaining('outcome: completed'), 300,
        scrollable: scrollable);
    expect(find.textContaining('outcome: completed'), findsOneWidget);

    await tester.scrollUntilVisible(find.textContaining('shipped it'), 300,
        scrollable: scrollable);
    expect(find.textContaining('shipped it'), findsWidgets);
  });
}
