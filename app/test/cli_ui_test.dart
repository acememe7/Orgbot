import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'package:orgbot_app/ui/sign_in_screen.dart';

void main() {
  testWidgets(
    'Linux setup and bot choices stay separate from Android accounts',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      FlutterSecureStorage.setMockInitialValues({});
      final library = Library(saver: MemorySaver());
      await library.load();
      addTearDown(library.dispose);
      library.addBot(
        BotDef(
          name: 'Legacy',
          handle: 'legacy',
          provider: ProviderKind.scripted,
        ),
      );
      final credentials = Credentials();
      await credentials.load();
      addTearDown(credentials.dispose);

      await tester.pumpWidget(
        MaterialApp(
          home: OrgBotScreen(
            library: library,
            credentials: credentials,
            desktopClis: true,
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('Setup'), findsWidgets);
      await tester.tap(find.text('Setup').last);
      await tester.pumpAndSettle();
      expect(find.text('Codex CLI'), findsOneWidget);
      expect(find.text('Claude Code'), findsOneWidget);
      expect(find.text('Grok CLI'), findsOneWidget);
      expect(find.byType(ListTile), findsNWidgets(3));
      expect(find.text('Accounts'), findsNothing);
      expect(find.byType(SignInScreen), findsNothing);

      await tester.tap(find.text('Bots'));
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('New bot'));
      await tester.pump(const Duration(milliseconds: 400));
      expect(find.text('API key'), findsNothing);
      expect(find.textContaining('Sign in'), findsNothing);
      final desktopPicker = tester.widget<DropdownButton<ProviderKind>>(
        find.byType(DropdownButton<ProviderKind>).first,
      );
      expect(desktopPicker.items!.map((item) => item.value), [
        ProviderKind.codex,
        ProviderKind.anthropic,
        ProviderKind.grok,
      ]);
      await tester.tap(find.text('Cancel'));
      await tester.pump(const Duration(milliseconds: 400));

      await tester.tap(find.text('Legacy'));
      await tester.pump(const Duration(milliseconds: 400));
      final legacyPicker = tester.widget<DropdownButton<ProviderKind>>(
        find.byType(DropdownButton<ProviderKind>).first,
      );
      final legacyItem = legacyPicker.items!.firstWhere(
        (item) => item.value == ProviderKind.scripted,
      );
      expect((legacyItem.child as Text).data, contains('unavailable here'));
      expect(find.text('API key'), findsNothing);
      await tester.tap(find.text('Save'));
      await tester.pump(const Duration(milliseconds: 400));
      expect(library.bots.single.provider, ProviderKind.scripted);

      await tester.pumpWidget(
        MaterialApp(
          home: OrgBotScreen(
            library: library,
            credentials: credentials,
            desktopClis: false,
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('Accounts'), findsWidgets);
      await tester.tap(find.text('Accounts').last);
      await tester.pumpAndSettle();
      expect(find.byType(SignInScreen), findsOneWidget);
      await tester.tap(find.text('Bots'));
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('New bot'));
      await tester.pump(const Duration(milliseconds: 400));
      final androidPicker = tester.widget<DropdownButton<ProviderKind>>(
        find.byType(DropdownButton<ProviderKind>).first,
      );
      expect(
        androidPicker.items!.map((item) => item.value),
        contains(ProviderKind.deepseek),
      );
      expect(
        androidPicker.items!.map((item) => item.value),
        contains(ProviderKind.codex),
      );
      expect(tester.takeException(), isNull);
    },
  );
}
