import 'dart:io';
import 'dart:ui' as ui;
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:orgbot_app/main.dart' show orgBotTheme;
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/model_catalog.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/room_file.dart';
import 'package:orgbot_app/state/room_archive.dart';
import 'package:orgbot_app/state/engine_room_controller.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'package:orgbot_app/ui/room_widgets.dart';

class RealHttp extends HttpOverrides {}

void main() {
  setUp(() {
    FlutterSecureStorage.setMockInitialValues({});
    SharedPreferences.setMockInitialValues({});
  });
  tearDown(() {
    ModelCatalog.credentials = null;
    ModelCatalog.invalidate();
  });

  test(
    'existing DeepSeek keys migrate once and new bots reuse the saved account',
    () async {
      final credentials = Credentials();
      final library = Library(saver: MemorySaver());
      await library.load();
      library.addBot(
        BotDef(
          name: 'Writer',
          provider: ProviderKind.deepseek,
          apiKey: 'private-test-key',
        ),
      );
      library.addBot(
        BotDef(
          name: 'Reviewer',
          provider: ProviderKind.deepseek,
          apiKey: 'private-test-key',
        ),
      );
      await library.shareExistingKeys(credentials);
      final restored = Credentials();
      await restored.load();
      final reload = Library(saver: library.saver);
      await reload.load();
      expect(
        restored.forProvider(ProviderKind.deepseek)?.token,
        'private-test-key',
      );
      expect(reload.bots.every((b) => b.apiKey.isEmpty), isTrue);
      final added = BotDef(
        name: 'New teammate',
        provider: ProviderKind.deepseek,
        model: 'deepseek-chat',
      );
      final file = buildRoomFile(
        RoomDef(name: 'Saved account'),
        [added],
        credentials: restored,
        canRunClis: false,
      );
      expect(file.skipped, isEmpty);
      expect(file.environment['DEEPSEEK_API_KEY'], 'private-test-key');
      expect(file.toml, isNot(contains('private-test-key')));
    },
  );

  testWidgets(
    'room navigation and restart preserve the real conversation',
    (tester) async {
      if (!Platform.environment.containsKey('ORGBOT_ENGINE')) return;
      final library = Library(saver: MemorySaver());
      await library.load();
      library.userName = 'Alex';
      tester.view.physicalSize = const Size(412, 892);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.binding.setSurfaceSize(const Size(412, 892));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      if (Platform.environment.containsKey('ORGBOT_UI_QA_DIR')) {
        await tester.runAsync(() async {
          final font = Platform.environment['ORGBOT_UI_FONT'];
          if (font != null) {
            final icons = File(
              '${File(font).parent.path}/MaterialIcons-Regular.otf',
            );
            if (await icons.exists()) {
              await (FontLoader('MaterialIcons')..addFont(
                    Future.value(
                      ByteData.sublistView(await icons.readAsBytes()),
                    ),
                  ))
                  .load();
            }
            await (FontLoader('Roboto')..addFont(
                  Future.value(
                    ByteData.sublistView(await File(font).readAsBytes()),
                  ),
                ))
                .load();
          }
        });
      }
      final canvas = GlobalKey();
      Future<void> capture(String name) async {
        final folder = Platform.environment['ORGBOT_UI_QA_DIR'];
        if (folder == null) return;
        await tester.runAsync(() async {
          final boundary =
              canvas.currentContext!.findRenderObject()
                  as RenderRepaintBoundary;
          final snapshot = await boundary.toImage(pixelRatio: 1.5);
          final bytes = await snapshot.toByteData(
            format: ui.ImageByteFormat.png,
          );
          await Directory(folder).create(recursive: true);
          await File(
            '$folder/$name.png',
          ).writeAsBytes(bytes!.buffer.asUint8List());
          snapshot.dispose();
        });
      }

      final bot = library.addBot(
        BotDef(name: 'Iris', handle: 'iris', provider: ProviderKind.scripted),
      );
      final room = library.addRoom(
        RoomDef(
          name: 'Continuity room',
          memory: 'Keep decisions brief and verify sources.',
          bots: [RoomSlot(bot.id)],
        ),
      );
      Future<void> draw() async {
        for (var i = 0; i < 6; i++) {
          await tester.pump(const Duration(milliseconds: 200));
        }
      }

      Future<void> waitFor(bool Function() done) async {
        await tester.runAsync(() async {
          for (var i = 0; i < 200 && !done(); i++) {
            await Future<void>.delayed(const Duration(milliseconds: 50));
          }
          expect(done(), isTrue, reason: library.sessions[room.id]?.error);
        });
        await draw();
      }

      try {
        await tester.runAsync(
          () => HttpOverrides.runWithHttpOverrides(
            () => library.session(room).start(),
            RealHttp(),
          ),
        );
        await tester.pumpWidget(
          RepaintBoundary(
            key: canvas,
            child: MaterialApp(
              debugShowCheckedModeBanner: false,
              theme: orgBotTheme(Brightness.dark),
              home: OrgBotScreen(library: library),
            ),
          ),
        );
        await tester.tap(find.text('Continuity room'));
        await tester.pump();
        await waitFor(() => library.sessions[room.id]?.isRunning == true);
        final original = library.sessions[room.id]!;
        await tester.runAsync(() async {
          original.say('Alex', 'Keep this decision after I leave.');
        });
        await waitFor(
          () => original.store!.messages.any(
            (m) =>
                m.body == 'Keep this decision after I leave.' &&
                !original.store!.isPending(m),
          ),
        );
        await capture('chat');
        await tester.tap(find.text('Memory'));
        await draw();
        await capture('memory');
        final id = original.store!.messages
            .firstWhere((m) => m.body == 'Keep this decision after I leave.')
            .id;
        await tester.pageBack();
        await draw();
        expect(original.isRunning, isTrue);
        await capture('rooms');
        await tester.tap(find.text('New room'));
        await draw();
        await capture('room-editor');
        expect(tester.takeException(), isNull);
        await tester.tap(find.text('Cancel'));
        await draw();
        await tester.tap(find.text('Bots'));
        await draw();
        await tester.tap(find.text('@iris'));
        await draw();
        await capture('bot-editor');
        expect(tester.takeException(), isNull);
        await tester.tap(find.text('Cancel'));
        await draw();
        await tester.tap(find.text('Rooms'));
        await draw();
        await tester.tap(find.text('Continuity room'));
        await draw();
        expect(identical(library.sessions[room.id], original), isTrue);
        expect(original.store!.messages.any((m) => m.id == id), isTrue);
        await tester.pageBack();
        await draw();
        await tester.runAsync(() => library.stopRoom(room.id));
        await tester.runAsync(
          () => HttpOverrides.runWithHttpOverrides(
            () => library.session(room).start(),
            RealHttp(),
          ),
        );
        await tester.tap(find.text('Continuity room'));
        await tester.pump();
        await waitFor(() => library.sessions[room.id]?.isRunning == true);
        final restored = library.sessions[room.id]!;
        await waitFor(() => restored.store!.messages.any((m) => m.id == id));
        expect(restored.store!.isPaused, isTrue);
        restored.phase = RoomLaunch.failed;
        restored.error = 'Could not load Grok models. Retry.';
        restored.store!.restore([
          {'type': 'error', 'message': restored.error},
        ]);
        await tester.tap(find.text('Work'));
        await draw();
        expect(find.textContaining('Showing saved history.'), findsOneWidget);
        expect(
          find.textContaining('Could not load Grok models.'),
          findsOneWidget,
        );
        expect(find.textContaining('Could not start'), findsOneWidget);
      } finally {
        await tester.pumpWidget(const SizedBox.shrink());
        await tester.runAsync(() async {
          await library.stopRoom(room.id);
          final dir = await RoomArchive.directory(room.id);
          if (await dir.exists()) await dir.delete(recursive: true);
        });
        library.dispose();
      }
    },
    skip: !Platform.environment.containsKey('ORGBOT_ENGINE'),
  );

  testWidgets('chat formatting renders without fetching model-provided images', (
    tester,
  ) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(
          body: SelectionArea(
            child: MessageText(
              text:
                  '**A decision**\n\n- Keep the room\n\n![remote](https://invalid.example/tracker.png)',
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('A decision', findRichText: true), findsWidgets);
    expect(find.byType(Image), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
