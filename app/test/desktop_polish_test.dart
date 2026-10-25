import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/main.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'package:orgbot_app/ui/engine_room_screen.dart';
import 'package:orgbot_app/orgbot/attention.dart';
import 'package:orgbot_app/state/room_store.dart';
import 'room_chat_ui_test.dart' show FakeClient, seeded;
import 'room_layout_test.dart' show PreviewRoom;

void main() {
  test('desktop text remains readable on both theme surfaces', () {
    for (final brightness in Brightness.values) {
      final theme = orgBotTheme(brightness, desktop: true);
      for (final style in [
        theme.textTheme.headlineLarge,
        theme.textTheme.titleMedium,
        theme.textTheme.bodyMedium,
      ]) {
        expect(style!.color, isNotNull);
        final text = style.color!.computeLuminance();
        for (final surface in [
          theme.colorScheme.surface,
          theme.colorScheme.surfaceContainerLow,
        ]) {
          final background = surface.computeLuminance();
          final contrast = text > background
              ? (text + .05) / (background + .05)
              : (background + .05) / (text + .05);
          expect(contrast, greaterThanOrEqualTo(4.5));
        }
      }
    }
  });
  testWidgets('filled desktop actions retain icon contrast', (tester) async {
    for (final brightness in Brightness.values) {
      final theme = orgBotTheme(brightness, desktop: true);
      await tester.pumpWidget(
        MaterialApp(
          theme: theme,
          home: Scaffold(
            body: IconButton.filled(
              onPressed: () {},
              icon: const Icon(Icons.add),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final color = IconTheme.of(tester.element(find.byIcon(Icons.add))).color!;
      final foreground = color.computeLuminance();
      final background = theme.colorScheme.primary.computeLuminance();
      final contrast = foreground > background
          ? (foreground + .05) / (background + .05)
          : (background + .05) / (foreground + .05);
      expect(contrast, greaterThanOrEqualTo(3));
    }
  });
  testWidgets('desktop tools collapse and navigation preserves the draft', (
    tester,
  ) async {
    final directory = Directory.systemTemp.createTempSync(
      'orgbot-polish-test-',
    );
    const pathProvider = MethodChannel('plugins.flutter.io/path_provider');
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      pathProvider,
      (_) async => directory.path,
    );
    addTearDown(() async {
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        pathProvider,
        null,
      );
      directory.deleteSync(recursive: true);
    });
    tester.view.physicalSize = const Size(1440, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final library = Library(saver: MemorySaver());
    await library.load();
    final room = RoomDef(id: 'polish_room', name: 'Desktop refinement');
    library.rooms.add(room);
    final store = seeded(FakeClient());
    library.sessions[room.id] = PreviewRoom(
      room: room,
      library: library,
      sample: store,
    );
    await tester.pumpWidget(
      MaterialApp(
        theme: orgBotTheme(Brightness.light, desktop: true),
        home: OrgBotScreen(library: library, desktopClis: true),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Desktop refinement'));
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 80)),
    );
    await tester.pumpAndSettle();
    expect(find.byTooltip('Collapse tools'), findsNothing);
    await tester.enterText(
      find.byType(TextField).first,
      'Keep this draft while I inspect the project.',
    );
    await tester.tap(find.byKey(const ValueKey('workspace-tool-0')));
    await tester.pumpAndSettle();
    expect(find.byTooltip('Collapse tools'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('collapse-workspace')));
    await tester.pumpAndSettle();
    expect(find.byTooltip('Collapse tools'), findsNothing);
    await tester.tap(find.text('Team').first);
    await tester.pumpAndSettle();
    expect(
      TickerMode.valuesOf(
        tester.element(find.byType(EngineRoomScreen, skipOffstage: false)),
      ).enabled,
      isFalse,
    );
    RoomStore.attentions.onRouteRequested?.call(
      AttentionTarget(
        roomId: room.id,
        reason: Attention.decision,
        eventId: 'owner-needed',
      ),
    );
    await tester.pumpAndSettle();
    expect(
      TickerMode.valuesOf(
        tester.element(find.byType(EngineRoomScreen)),
      ).enabled,
      isTrue,
    );
    expect(
      find.text('Keep this draft while I inspect the project.'),
      findsOneWidget,
    );
    await tester.tap(find.byKey(const ValueKey('toggle-sidebar')));
    await tester.pumpAndSettle();
    expect(find.byTooltip('Expand sidebar (Ctrl+B)'), findsOneWidget);
    for (final width in [800.0, 375.0]) {
      tester.view.physicalSize = Size(width, 800);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('workspace-tool-0')));
      await tester.pumpAndSettle();
      expect(find.byTooltip('Collapse tools'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('collapse-workspace')));
      await tester.pumpAndSettle();
      expect(
        find.text('Keep this draft while I inspect the project.'),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    }
    await tester.pumpWidget(
      MaterialApp(
        theme: orgBotTheme(Brightness.light, desktop: true),
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(
            context,
          ).copyWith(textScaler: const TextScaler.linear(1.6)),
          child: child!,
        ),
        home: OrgBotScreen(library: library, desktopClis: true),
      ),
    );
    await tester.pump(const Duration(milliseconds: 300));
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
    library.dispose();
    store.dispose();
  });
}
