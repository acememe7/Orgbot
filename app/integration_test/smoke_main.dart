// Standalone phone check when Wi-Fi ADB cannot keep the test debugger attached.
// Build only as com.orgbot.app.qa with the isolated SSH fixture defines.
import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'dart:ui' as ui;
import 'package:orgbot_app/main.dart' show orgBotTheme;
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/state/room_archive.dart';
import 'package:orgbot_app/state/background_rooms.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'package:orgbot_app/ui/engine_room_screen.dart';
import 'package:orgbot_app/engine/embedded_engine.dart';
import 'package:orgbot_app/workspace/workspace.dart';

void check(bool ok, String message) {
  if (!ok) throw StateError(message);
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final library = Library(saver: MemorySaver());
  await library.load();
  library.userName = 'Alex';
  final credentials = Credentials();
  await credentials.load();
  final bot = library.addBot(
    BotDef(name: 'Iris', handle: 'iris', provider: ProviderKind.scripted),
  );
  final room = library.addRoom(
    RoomDef(
      id: 'continuity_qa',
      name: 'Product research',
      goal: 'Compare the options and keep the decisions here.',
      memory:
          'Keep recommendations brief. Verify sources before making a decision.',
      bots: [RoomSlot(bot.id)],
    ),
  );
  library.addRoom(
    RoomDef(
      name: 'Release planning',
      goal: 'Prepare the next Android build.',
      bots: [RoomSlot(bot.id)],
    ),
  );
  final navigator = GlobalKey<NavigatorState>();
  final canvas = GlobalKey();
  runApp(
    RepaintBoundary(
      key: canvas,
      child: MaterialApp(
        navigatorKey: navigator,
        debugShowCheckedModeBanner: false,
        theme: orgBotTheme(Brightness.dark),
        home: OrgBotScreen(library: library, credentials: credentials),
      ),
    ),
  );
  final base = await WorkspaceConnection.phoneBase();
  final result = File('$base/smoke-result.json');
  final passed = <String>[];
  Future<void> progress(String stage) => result.writeAsString(
    jsonEncode({'passed': passed, 'stage': stage}),
    flush: true,
  );
  WorkspaceConnection? local, remote;
  Future<void> waitFor(bool Function() done, String label) async {
    for (var i = 0; i < 200 && !done(); i++) {
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
    check(done(), label);
  }

  Future<void> capture(String name) async {
    await Future<void>.delayed(const Duration(milliseconds: 700));
    try {
      await WidgetsBinding.instance.endOfFrame.timeout(
        const Duration(seconds: 5),
      );
      final boundary =
          canvas.currentContext!.findRenderObject() as RenderRepaintBoundary;
      final snapshot = await boundary.toImage(pixelRatio: 1.5);
      final bytes = await snapshot.toByteData(format: ui.ImageByteFormat.png);
      await File('$base/$name.png').writeAsBytes(bytes!.buffer.asUint8List());
      snapshot.dispose();
    } catch (_) {
      /* A locked device may not render; execution checks still run. */
    }
  }

  final folder = await Directory('$base/smoke').create(recursive: true);
  try {
    await BackgroundRooms.update('smoke', true);
    await capture('rooms');
    await progress('phone files');
    local = WorkspaceConnection(WorkspaceConfig(path: folder.path), 'smoke');
    await local.connect();
    await local.execute('workspace_write', {
      'path': 'check.txt',
      'content': 'phone check',
    });
    check(
      await local.execute('workspace_read', {'path': 'check.txt'}) ==
          'phone check',
      'Phone read/write',
    );
    passed.add('phone file read/write');
    await progress('native engine');
    final engine = EmbeddedEngine.load();
    final handle = engine.openRoom('''
[room]
name = "phone smoke"
goal = "check the native engine"
completion = "manual"
[[bots]]
handle = "demo"
provider = { kind = "scripted", script = "native reply" }
''', {});
    try {
      check(engine.say(handle, 'QA', '@demo hello'), 'Native input rejected');
      var replied = false, directed = false;
      for (var i = 0; i < 100 && !(replied && directed); i++) {
        for (final frame in engine.poll(handle)) {
          final event = (frame['envelope'] as Map?)?['event'] as Map?;
          final message = event?['message'] as Map?;
          if (message?['body'] == 'native reply') replied = true;
          if (message?['body'] == '@demo hello') {
            directed = (message?['audience'] as Map?)?['kind'] == 'agents';
          }
        }
        await Future<void>.delayed(const Duration(milliseconds: 100));
      }
      check(replied && directed, 'Native reply / tag routing');
      final beforeRenewal = engine.snapshot(handle);
      check(
        engine.control(handle, {
          'type': 'update_credentials',
          'environment': {'GROK_AUTH_TOKEN': 'qa-renewed-token'},
        }),
        'Native credential update rejected',
      );
      check(
        jsonEncode(engine.snapshot(handle)?['roster']) ==
            jsonEncode(beforeRenewal?['roster']),
        'Credential update replaced the bots',
      );
      check(
        !jsonEncode(engine.poll(handle)).contains('qa-renewed-token'),
        'Credentials leaked into the room journal',
      );
      check(
        engine.say(handle, 'QA', '@demo after renewal'),
        'Native input after renewal rejected',
      );
      replied = false;
      for (var i = 0; i < 100 && !replied; i++) {
        for (final frame in engine.poll(handle)) {
          final message =
              ((frame['envelope'] as Map?)?['event'] as Map?)?['message']
                  as Map?;
          if (message?['body'] == 'native reply') replied = true;
        }
        await Future<void>.delayed(const Duration(milliseconds: 100));
      }
      check(replied, 'Room stopped responding after credential update');
      passed.add('native engine reply and tag routing');
      passed.add('native credential update preserves the running room');
    } finally {
      engine.closeRoom(handle);
    }
    await progress('SSH');
    remote = WorkspaceConnection(
      WorkspaceConfig(
        kind: 'ssh',
        host: '127.0.0.1',
        port: 22222,
        username: 'qa',
        password: const String.fromEnvironment('SSH_TEST_PASSWORD'),
        path: const String.fromEnvironment('SSH_TEST_PATH'),
        fingerprint: const String.fromEnvironment('SSH_TEST_FINGERPRINT'),
        commands: true,
      ),
      'smoke',
    );
    await remote.connect();
    await remote.execute('workspace_write', {
      'path': 'phone-smoke.txt',
      'content': 'remote check',
    });
    final command =
        await remote.execute('workspace_run', {'command': 'printf shell-ok'})
            as Map;
    check(
      command['exit_code'] == 0 && command['output'] == 'shell-ok',
      'SSH shell',
    );
    await remote.cancelOperation();
    check(!remote.connected, 'Disconnected state');
    check(
      await remote.execute('workspace_read', {'path': 'phone-smoke.txt'}) ==
          'remote check',
      'SSH automatic reconnect',
    );
    check(remote.connected, 'Reconnected state');
    passed.add('SSH file tools, command and pinned reconnect');
    await progress('room continuity');
    final controller = library.session(room);
    await controller.start();
    check(controller.isRunning, controller.error ?? 'Room did not start');
    controller.say(
      'Alex',
      '@iris **Decision:** Keep room history and use the selected working folder.',
    );
    await waitFor(
      () => controller.store!.messages.any(
        (m) =>
            m.body.contains('**Decision:**') && !controller.store!.isPending(m),
      ),
      'Message was not committed',
    );
    final id = controller.store!.messages
        .firstWhere((m) => m.body.contains('**Decision:**'))
        .id;
    navigator.currentState!.push(
      MaterialPageRoute<void>(
        builder: (_) => EngineRoomScreen(room: room, library: library),
      ),
    );
    await capture('chat');
    navigator.currentState!.pop();
    await Future<void>.delayed(const Duration(milliseconds: 500));
    check(
      identical(controller, library.session(room)) && controller.isRunning,
      'Navigation stopped the room',
    );
    passed.add('leave room without stopping or losing chat');
    // The host backgrounds the QA app during this window. Heartbeats and SSH
    // requests must still complete with the display asleep.
    await progress('background window');
    for (var i = 0; i < 18; i++) {
      await Future<void>.delayed(const Duration(seconds: 2));
      await File('$base/heartbeat.json').writeAsString(
        jsonEncode({'tick': i + 1, 'time': DateTime.now().toIso8601String()}),
        flush: true,
      );
      if (i == 8) {
        await remote.cancelOperation();
        check(
          await remote.execute('workspace_read', {'path': 'phone-smoke.txt'}) ==
              'remote check',
          'Background SSH reconnect',
        );
        controller.say('Alex', 'Saved while the app was in the background.');
      }
    }
    await waitFor(
      () => controller.store!.messages.any(
        (m) =>
            m.body == 'Saved while the app was in the background.' &&
            !controller.store!.isPending(m),
      ),
      'Background message was not committed',
    );
    passed.add('background timer, native room input and SSH reconnect');
    await library.stopRoom(room.id);
    final restored = library.session(room);
    await restored.start();
    await waitFor(
      () => restored.store!.messages.any((m) => m.id == id),
      'Saved chat was not restored',
    );
    check(
      restored.store!.isPaused,
      'Restored room must wait for explicit resume',
    );
    final state =
        jsonDecode(await (await RoomArchive.stateFile(room.id)).readAsString())
            as Map;
    check((state['bots'] as Map).isNotEmpty, 'Bot memory was not saved');
    passed.add('native checkpoint restores same conversation and bot memory');
    await progress('foreground screenshots');
    navigator.currentState!.push(
      MaterialPageRoute<void>(
        builder: (_) => EngineRoomScreen(room: room, library: library),
      ),
    );
    await capture('restored-chat');
    void selectMemory(Element element) {
      if (element.widget is TabBar) {
        DefaultTabController.of(element).animateTo(3);
      }
      element.visitChildElements(selectMemory);
    }

    canvas.currentContext!.visitChildElements(selectMemory);
    await capture('memory');
    await library.stopRoom(room.id);
    await result.writeAsString(
      jsonEncode({'passed': passed, 'ok': true}),
      flush: true,
    );
  } catch (e) {
    await result.writeAsString(
      jsonEncode({
        'passed': passed,
        'ok': false,
        'error_type': e.runtimeType.toString(),
        'error': e.toString().replaceAll(
          const String.fromEnvironment('SSH_TEST_PASSWORD'),
          '[fixture]',
        ),
      }),
      flush: true,
    );
  } finally {
    await local?.close();
    await remote?.close();
    await library.stopRoom(room.id);
    await BackgroundRooms.update('smoke', false);
    await folder.delete(recursive: true);
  }
}
