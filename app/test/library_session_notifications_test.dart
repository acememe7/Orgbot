import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/engine/core_server.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/state/engine_room_controller.dart';

class _RealHttpOverrides extends HttpOverrides {}

Future<bool> _waitFor(bool Function() check) async {
  final deadline = DateTime.now().add(const Duration(seconds: 30));
  while (DateTime.now().isBefore(deadline)) {
    if (check()) return true;
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }
  return check();
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final engine = findEngine();

  test(
    'Library session notifications track room previews and detach on stop',
    () async {
      final fixture = await Directory.systemTemp.createTemp(
        "orgbot-notifications-",
      );
      addTearDown(() => fixture.delete(recursive: true));
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      const pathProvider = MethodChannel('plugins.flutter.io/path_provider');
      messenger.setMockMethodCallHandler(
        pathProvider,
        (call) async => fixture.path,
      );
      final previousOverrides = HttpOverrides.current;
      HttpOverrides.global = _RealHttpOverrides();
      addTearDown(() {
        messenger.setMockMethodCallHandler(pathProvider, null);
        HttpOverrides.global = previousOverrides;
      });
      final library = Library(saver: MemorySaver());
      final bot = BotDef(
        name: 'Helper',
        handle: 'helper',
        provider: ProviderKind.scripted,
        role: 'writer',
      );
      library.bots.add(bot);
      final room = RoomDef(
        name: 'Notification check',
        bots: [RoomSlot(bot.id)],
      );
      final session = library.session(room);
      var shellNotifications = 0;
      library.addListener(() => shellNotifications++);
      addTearDown(() async {
        await session.stop();
        library.dispose();
      });

      await session.start();
      expect(session.phase, RoomLaunch.running, reason: session.error);
      expect(
        shellNotifications,
        greaterThan(0),
        reason: 'phase/run-state changes must reach Library listeners',
      );
      expect(
        await _waitFor(() => session.store?.agents.isNotEmpty == true),
        isTrue,
      );

      // Repeated controller notifications with the same sidebar summary are
      // private to the room and must not rebuild the Library shell.
      final beforeNoOp = shellNotifications;
      session.notifyListeners();
      session.notifyListeners();
      expect(shellNotifications, beforeNoOp);

      await session.say('Tester', 'hello from the room');
      expect(
        await _waitFor(
          () =>
              session.store?.messages.any(
                (m) => m.body == 'hello from the room',
              ) ==
              true,
        ),
        isTrue,
      );
      expect(
        shellNotifications,
        greaterThan(beforeNoOp),
        reason: 'latest-message preview changes must reach Library listeners',
      );

      final beforeStop = shellNotifications;
      await library.stopRoom(room.id);
      expect(shellNotifications, beforeStop + 1);
    },
    skip: engine.available
        ? false
        : 'engine binary unavailable: ${engine.problem}',
    timeout: const Timeout(Duration(seconds: 90)),
  );
}
