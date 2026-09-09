import 'dart:io';
import 'dart:convert';
import 'dart:ui' as ui;
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/main.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/state/engine_room_controller.dart';
import 'package:orgbot_app/state/room_store.dart';
import 'package:orgbot_app/ui/engine_room_screen.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'room_chat_ui_test.dart' show FakeClient, seeded, event;

class PreviewRoom extends EngineRoomController {
  PreviewRoom({
    required super.room,
    required super.library,
    required this.sample,
  }) {
    phase = RoomLaunch.running;
  }
  final RoomStore sample;
  @override
  RoomStore get store => sample;
  @override
  Future<void> start() async {}
}

void main() {
  testWidgets(
    'room and editor fit small screens and enlarged text; tabs retain history',
    (tester) async {
      final shots = Platform.environment['ORGBOT_UI_SHOTS'];
      if (shots != null) {
        final root = Platform.environment['FLUTTER_ROOT']!;
        final font = FontLoader('Inter')
          ..addFont(
            Future.value(
              ByteData.sublistView(
                File('assets/fonts/Inter.ttf').readAsBytesSync(),
              ),
            ),
          );
        await tester.runAsync(font.load);
        final icons = FontLoader('MaterialIcons')
          ..addFont(
            Future.value(
              ByteData.sublistView(
                File(
                  '$root/bin/cache/artifacts/material_fonts/MaterialIcons-Regular.otf',
                ).readAsBytesSync(),
              ),
            ),
          );
        await tester.runAsync(icons.load);
      }
      tester.view.physicalSize = const Size(390, 844);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final client = FakeClient();
      final store = seeded(client);
      FlutterSecureStorage.setMockInitialValues({});
      SharedPreferences.setMockInitialValues({});
      final credentials = Credentials();
      await credentials.load();
      await credentials.importBundle(
        jsonEncode({
          'version': 1,
          'providers': [
            {
              'provider': 'codex',
              'token': 'fixture-only',
              'account': 'alex@example.test',
              'plan': 'Plus',
            },
            {
              'provider': 'grok',
              'token': 'fixture-only',
              'account': 'alex@example.test',
            },
          ],
        }),
      );
      addTearDown(credentials.dispose);
      final library = Library(saver: MemorySaver());
      await library.load();
      addTearDown(store.dispose);
      addTearDown(library.dispose);
      final bot = library.addBot(
        BotDef(name: 'Iris', handle: 'iris', provider: ProviderKind.scripted),
      );
      final room = library.addRoom(
        RoomDef(
          name: 'Aether research',
          goal:
              'Understand how researchers collect and evaluate their sources.',
          bots: [RoomSlot(bot.id)],
        ),
      );
      library.sessions[room.id] = PreviewRoom(
        room: room,
        library: library,
        sample: store,
      );
      final messages = [
        'Review the research brief and recommend a focused direction. Keep the evidence and assumptions separate.',
        '## A clearer starting point\n\nThe strongest opportunity is in the work between collecting a source and using it.\n\n- **Audience:** independent researchers\n- **Problem:** notes scattered across documents\n- **Next step:** test the workflow with five people\n\nI’ll compare the alternatives and save the findings in our working folder.',
        'I’m reviewing the evidence behind each claim. The source list is ready for a closer look.',
      ];
      for (var i = 0; i < messages.length; i++) {
        client.emit(
          event({
            'event': 'message_posted',
            'message': {
              'id': 'm$i',
              'room': 'r',
              'author': i == 0
                  ? {'kind': 'human', 'name': 'Alex'}
                  : {'kind': 'agent', 'agent': i == 1 ? 'bot_a' : 'bot_b'},
              'audience': {'kind': 'everyone'},
              'intent': 'say',
              'body': messages[i],
              'sent_at': 1788672000000 + i * 60000,
            },
          }, seq: i + 1),
        );
      }
      final canvas = GlobalKey();
      Widget app(
        Widget home, {
        double scale = 1,
        Brightness brightness = Brightness.dark,
      }) => RepaintBoundary(
        key: canvas,
        child: MaterialApp(
          debugShowCheckedModeBanner: false,
          theme: orgBotTheme(brightness),
          builder: (context, child) => MediaQuery(
            data: MediaQuery.of(
              context,
            ).copyWith(textScaler: TextScaler.linear(scale)),
            child: child!,
          ),
          home: home,
        ),
      );
      Future<void> capture(String name) async {
        if (shots == null) return;
        await tester.runAsync(() async {
          final image =
              await (canvas.currentContext!.findRenderObject()
                      as RenderRepaintBoundary)
                  .toImage(pixelRatio: 2);
          final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
          await File(
            '$shots/$name.png',
          ).writeAsBytes(bytes!.buffer.asUint8List());
          image.dispose();
        });
      }

      await tester.pumpWidget(
        app(EngineRoomScreen(room: room, library: library)),
      );
      await tester.pumpAndSettle();
      await capture('chat-dark');
      await tester.drag(find.byType(CustomScrollView), const Offset(0, 300));
      await tester.pumpAndSettle();
      final scroll = tester
          .widget<CustomScrollView>(find.byType(CustomScrollView))
          .controller!;
      final before = scroll.offset;
      await tester.tap(find.text('Work'));
      await tester.pumpAndSettle();
      await capture('work-dark');
      await tester.tap(find.text('Chat'));
      await tester.pumpAndSettle();
      expect(scroll.offset, closeTo(before, 1));
      for (final brightness in Brightness.values) {
        await tester.pumpWidget(
          app(
            EngineRoomScreen(room: room, library: library),
            scale: 1.6,
            brightness: brightness,
          ),
        );
        tester.view.physicalSize = const Size(360, 780);
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
        await tester.tap(find.text('Work'));
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
      }
      tester.view.physicalSize = const Size(390, 844);
      await tester.pumpWidget(
        app(OrgBotScreen(library: library, credentials: credentials)),
      );
      await tester.pumpAndSettle();
      await capture('rooms-dark');
      await tester.tap(find.text('Bots'));
      await tester.pumpAndSettle();
      await capture('team-dark');
      await tester.tap(find.byTooltip('New bot'));
      await tester.pumpAndSettle();
      await capture('editor-dark');
      expect(tester.takeException(), isNull);
      await tester.tap(find.byType(CloseButton));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Accounts'));
      await tester.pumpAndSettle();
      await capture('accounts-dark');
      await tester.tap(find.text('Rooms').last);
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('New room'));
      await tester.pumpAndSettle();
      await capture('room-editor-dark');
      await tester.tap(find.byType(CloseButton));
      await tester.pumpAndSettle();
      await tester.pumpWidget(
        app(
          OrgBotScreen(library: library, credentials: credentials),
          brightness: Brightness.light,
        ),
      );
      await tester.pumpAndSettle();
      await capture('rooms-light');
      for (final brightness in Brightness.values) {
        tester.view.physicalSize = const Size(360, 780);
        await tester.pumpWidget(
          app(
            OrgBotScreen(library: library, credentials: credentials),
            brightness: brightness,
            scale: 1.6,
          ),
        );
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
        await tester.tap(find.text('Bots'));
        await tester.pumpAndSettle();
        await tester.tap(find.byTooltip('New bot'));
        await tester.pumpAndSettle();
        await capture('editor-large');
        expect(tester.takeException(), isNull);
        await tester.tap(find.byType(CloseButton));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Accounts'));
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
        await tester.tap(find.text('Rooms').last);
        await tester.pumpAndSettle();
      }
    },
  );
}
