// The path the app actually takes: generate a room file, start the real
// engine, connect, say something, and get an answer back.
//
// Skipped when the engine binary is not built, so a checkout without it still
// runs the suite. Build it with: (cd core && cargo build --release)
//
// ignore_for_file: prefer_interpolation_to_compose_strings

import 'dart:io';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/engine/core_server.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/state/engine_room_controller.dart';

/// Wait for [check] to hold, or give up. Returns whether it held.
Future<bool> waitFor(
  bool Function() check, {
  Duration timeout = const Duration(seconds: 25),
}) async {
  final deadline = DateTime.now().add(timeout);
  while (DateTime.now().isBefore(deadline)) {
    if (check()) return true;
    await Future<void>.delayed(const Duration(milliseconds: 100));
  }
  return check();
}

class _RealHttpOverrides extends HttpOverrides {}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final engine = findEngine();

  group(
    'a room on the real engine',
    () {
      late EngineRoomController controller;

      late Directory fixture;
      HttpOverrides? previousOverrides;
      const pathProvider = MethodChannel('plugins.flutter.io/path_provider');
      setUp(() async {
        fixture = await Directory.systemTemp.createTemp('orgbot-e2e-');
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(
              pathProvider,
              (call) async => fixture.path,
            );
        previousOverrides = HttpOverrides.current;
        HttpOverrides.global = _RealHttpOverrides();
        final library = Library();
        // Scripted, so the test needs no credentials and no CLI.
        final bot = BotDef(
          name: 'Helper',
          handle: 'helper',
          provider: ProviderKind.scripted,
          role: 'writer',
        );
        library.bots.add(bot);
        final room = RoomDef(
          name: 'debug',
          goal: 'answer the person',
          bots: [RoomSlot(bot.id)],
        );
        controller = EngineRoomController(room: room, library: library);
      });

      tearDown(() async {
        await controller.stop();
        controller.dispose();
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(pathProvider, null);
        HttpOverrides.global = previousOverrides;
        await fixture.delete(recursive: true);
      });

      test(
        'starts, connects, and shows what you say',
        () async {
          await controller.start();

          expect(
            controller.phase,
            RoomLaunch.running,
            reason: 'the room should reach running; got: ${controller.error}',
          );
          final store = controller.store;
          expect(store, isNotNull);

          // Wait for the roster to arrive in the snapshot.
          expect(
            await waitFor(() => store!.agents.isNotEmpty),
            isTrue,
            reason: 'the snapshot should carry the roster',
          );

          controller.say('Tester', 'hello there');

          // What the person said must appear in the transcript.
          final mine = await waitFor(
            () => store!.messages.any((m) => m.body == 'hello there'),
          );
          expect(
            mine,
            isTrue,
            reason:
                'your own message must show up in the room; '
                'transcript was: ${store!.messages.map((m) => m.body).toList()}',
          );

          // And a bot must answer it.
          final reply = await waitFor(
            () => store.messages.any((m) => m.author.isAgent),
          );
          expect(
            reply,
            isTrue,
            reason:
                'a bot should reply; transcript was: '
                '${store.messages.map((m) => "${m.author.kind}:${m.body}").toList()}',
          );
        },
        timeout: const Timeout(Duration(seconds: 90)),
      );
    },
    skip: engine.available
        ? false
        : 'engine binary not found: ${engine.problem}',
  );
}
