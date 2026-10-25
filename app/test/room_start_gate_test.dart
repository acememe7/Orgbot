import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/state/engine_room_controller.dart';
import 'package:orgbot_app/state/room_archive.dart';
import 'package:orgbot_app/ui/engine_room_screen.dart';

class CountingRoom extends EngineRoomController {
  CountingRoom({
    required super.room,
    required super.library,
    required RoomLaunch phase,
  }) {
    this.phase = phase;
  }

  int starts = 0;

  @override
  Future<void> start() async {
    starts++;
    phase = RoomLaunch.generating;
    notifyListeners();
  }
}

Future<void> expectFreshRoomGate(WidgetTester tester, RoomLaunch phase) async {
  final library = Library(saver: MemorySaver());
  await library.load();
  final id =
      'start_gate_${phase.name}_${DateTime.now().microsecondsSinceEpoch}';
  final room = RoomDef(name: 'Fresh room', id: id, maxTurns: 0);
  final controller = CountingRoom(room: room, library: library, phase: phase);
  library.sessions[id] = controller;

  await tester.pumpWidget(
    MaterialApp(
      home: EngineRoomScreen(room: room, library: library),
    ),
  );
  await tester.pump();
  await tester.runAsync(() async {
    await RoomArchive.stateFile(id);
    await Future<void>.delayed(const Duration(milliseconds: 80));
  });
  for (var i = 0; i < 10; i++) {
    await tester.pump(const Duration(milliseconds: 20));
    final button = tester.widget<FilledButton>(
      find.byKey(const ValueKey('start-fresh-room')),
    );
    if (button.onPressed != null) break;
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 30)),
    );
  }
  expect(find.text('Ready to start this room?'), findsOneWidget);
  expect(
    find.textContaining(
      'Work continues until the task is finished or you pause',
    ),
    findsOneWidget,
  );
  expect(
    find.textContaining('idle sessions wait without model calls'),
    findsOneWidget,
  );
  final startButton = find.byKey(const ValueKey('start-fresh-room'));
  expect(tester.widget<FilledButton>(startButton).onPressed, isNotNull);
  expect(controller.starts, 0);

  await tester.tap(startButton);
  await tester.pump();
  expect(controller.starts, 1);
  expect(find.text('Preparing the room'), findsOneWidget);
  expect(tester.takeException(), isNull);

  await tester.pumpWidget(const SizedBox.shrink());
  library.dispose();
  await tester.runAsync(() async {
    final dir = await RoomArchive.directory(id);
    if (await dir.exists()) await dir.delete(recursive: true);
  });
}

void main() {
  testWidgets('fresh desktop rooms require one explicit start', (tester) async {
    if (!Platform.isLinux) return;
    for (final phase in [
      RoomLaunch.idle,
      RoomLaunch.closed,
      RoomLaunch.failed,
    ]) {
      await expectFreshRoomGate(tester, phase);
    }
  });
}
