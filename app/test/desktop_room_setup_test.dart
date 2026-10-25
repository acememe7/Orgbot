import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';

void main() {
  testWidgets('desktop sessions run continuously and keep Android defaults', (
    tester,
  ) async {
    Future<void> openRoomEditor(bool desktopClis) async {
      final library = Library(saver: MemorySaver());
      await library.load();
      await tester.pumpWidget(
        MaterialApp(
          home: OrgBotScreen(library: library, desktopClis: desktopClis),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(
        desktopClis
            ? find.text('New session').first
            : find.byTooltip('New room'),
      );
      await tester.pumpAndSettle();

      final fields = tester
          .widgetList<DropdownButton<int>>(find.byType(DropdownButton<int>))
          .toList();
      expect(fields.first.value, desktopClis ? 0 : 2000);
      expect(
        fields.first.items!.map((item) => item.value),
        containsAll(desktopClis ? [0, 100, 500] : [500, 2000, 5000, 20000, 0]),
      );
      expect(fields[1].value, desktopClis ? 4 : 2);
      await tester.pumpWidget(const SizedBox.shrink());
      library.dispose();
    }

    await openRoomEditor(true);
    await openRoomEditor(false);
    expect(tester.takeException(), isNull);
  });

  testWidgets('review only forces inspect and disables create and deliver', (
    tester,
  ) async {
    final library = Library(saver: MemorySaver());
    await library.load();
    addTearDown(library.dispose);
    await tester.pumpWidget(
      MaterialApp(home: OrgBotScreen(library: library, desktopClis: true)),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Team').first);
    await tester.pump();
    await tester.tap(find.byTooltip('New bot'));
    await tester.pump(const Duration(milliseconds: 400));

    final reviewOnly = find.widgetWithText(FilterChip, 'Review only');
    await tester.ensureVisible(reviewOnly);
    await tester.tap(reviewOnly);
    await tester.pump();

    final create = tester.widget<FilterChip>(
      find.widgetWithText(FilterChip, 'Creates tasks'),
    );
    final inspect = tester.widget<FilterChip>(
      find.widgetWithText(FilterChip, 'Inspects work'),
    );
    final deliver = tester.widget<FilterChip>(
      find.widgetWithText(FilterChip, 'Delivers'),
    );
    expect(create.selected, isFalse);
    expect(create.onSelected, isNull);
    expect(inspect.selected, isTrue);
    expect(deliver.selected, isFalse);
    expect(deliver.onSelected, isNull);

    final nameField = find.byWidgetPredicate(
      (widget) => widget is TextField && widget.decoration?.labelText == 'Name',
    );
    await tester.ensureVisible(nameField);
    await tester.enterText(nameField, 'Reviewer');
    await tester.tap(find.text('Save').last);
    await tester.pump(const Duration(milliseconds: 400));
    final powers = library.bots.single.powers!;
    expect(powers.reviewOnly, isTrue);
    expect(powers.mayInspect, isTrue);
    expect(powers.mayCreateTasks, isFalse);
    expect(powers.maySubmit, isFalse);
    expect(tester.takeException(), isNull);
  });
}
