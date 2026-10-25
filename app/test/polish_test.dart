import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:orgbot_app/main.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'package:orgbot_app/ui/polish.dart';
import 'package:orgbot_app/ui/sign_in_screen.dart';

void main() {
  testWidgets(
    'press feedback cancels on a drag and reduced motion is stationary',
    (tester) async {
      var opened = 0;
      Widget surface(bool reduced) => MaterialApp(
        home: MediaQuery(
          data: MediaQueryData(disableAnimations: reduced),
          child: Scaffold(
            body: ListView(
              children: [
                PressSurface(
                  onTap: () => opened++,
                  child: const SizedBox(height: 300, child: Text('Room')),
                ),
              ],
            ),
          ),
        ),
      );
      await tester.pumpWidget(surface(false));
      final gesture = await tester.startGesture(
        tester.getCenter(find.text('Room')),
      );
      await tester.pump(const Duration(milliseconds: 150));
      expect(
        tester.widget<AnimatedScale>(find.byType(AnimatedScale)).scale,
        lessThan(1),
      );
      await gesture.moveBy(const Offset(0, -100));
      await gesture.up();
      await tester.pumpAndSettle();
      expect(opened, 0);
      expect(tester.widget<AnimatedScale>(find.byType(AnimatedScale)).scale, 1);
      await tester.pumpWidget(surface(true));
      expect(
        tester.widget<AnimatedScale>(find.byType(AnimatedScale)).duration,
        Duration.zero,
      );
      await tester.tap(find.text('Room'));
      await tester.pumpAndSettle();
      expect(opened, 1);
    },
  );

  testWidgets(
    'navigation retains search and account state, including rapid changes and large text',
    (tester) async {
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
            },
          ],
        }),
      );
      final library = Library(saver: MemorySaver());
      await library.load();
      library.addBot(
        BotDef(name: 'Iris', handle: 'iris', provider: ProviderKind.scripted),
      );
      library.addRoom(RoomDef(name: 'Research'));
      tester.view.physicalSize = const Size(360, 780);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      Widget app(bool reduced) => MaterialApp(
        theme: orgBotTheme(Brightness.light),
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(context).copyWith(
            disableAnimations: reduced,
            textScaler: const TextScaler.linear(1.6),
          ),
          child: child!,
        ),
        home: OrgBotScreen(
          library: library,
          credentials: credentials,
          desktopClis: false,
        ),
      );
      await tester.pumpWidget(app(false));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('search-0')),
        'Research',
      );
      await tester.tap(find.text('Accounts'));
      await tester.pumpAndSettle();
      final accountState = tester.state(find.byType(SignInScreen));
      expect(find.text('alex@example.test'), findsOneWidget);
      expect(find.text('0 model(s)'), findsNothing);
      await tester.tap(
        find.byTooltip('${ProviderKind.codex.label} account options'),
      );
      await tester.pumpAndSettle();
      expect(find.text('Reconnect'), findsOneWidget);
      expect(find.text('Sign out'), findsOneWidget);
      await tester.tapAt(const Offset(10, 10));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Rooms').last);
      await tester.pump(const Duration(milliseconds: 50));
      await tester.tap(find.text('Bots'));
      await tester.pumpAndSettle();
      expect(find.text('Team'), findsOneWidget);
      await tester.tap(find.text('Rooms').last);
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<TextField>(find.byKey(const ValueKey('search-0')))
            .controller!
            .text,
        'Research',
      );
      await tester.pumpWidget(app(true));
      await tester.tap(find.text('Accounts'));
      await tester.pumpAndSettle();
      expect(tester.state(find.byType(SignInScreen)), same(accountState));
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
      library.dispose();
      credentials.dispose();
    },
  );
}
