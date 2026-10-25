import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/model_catalog.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/oauth/device_auth.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'package:orgbot_app/ui/polish.dart';
import 'package:orgbot_app/ui/sign_in_screen.dart';

Future<void> pumpUiTransition(WidgetTester tester) async {
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 250));
}

void main() {
  testWidgets(
    'desktop navigation stays mounted and team rows compact at width',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      FlutterSecureStorage.setMockInitialValues({});
      tester.view.physicalSize = const Size(800, 700);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      final library = Library(saver: MemorySaver());
      await library.load();
      library.addBot(
        BotDef(
          name: 'A deliberately long teammate name that must remain readable',
          handle: 'long-team-mate',
          provider: ProviderKind.codex,
          model: 'gpt-5.6-sol',
        ),
      );
      addTearDown(library.dispose);

      await tester.pumpWidget(
        MaterialApp(home: OrgBotScreen(library: library, desktopClis: true)),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Team').first);
      await tester.pump();
      expect(
        find.byKey(ValueKey('desktop-bot-${library.bots.single.id}')),
        findsOneWidget,
      );

      await tester.enterText(
        find.byKey(const ValueKey('search-1')),
        'long-team',
      );
      await tester.pump();
      expect(find.textContaining('deliberately long teammate'), findsOneWidget);

      await tester.tap(find.text('Connections & tools').first);
      await tester.pump();
      await tester.tap(find.text('Team').first);
      await tester.pump();
      expect(
        tester
            .widget<TextField>(find.byKey(const ValueKey('search-1')))
            .controller!
            .text,
        'long-team',
      );

      tester.view.physicalSize = const Size(620, 700);
      await tester.pump();
      expect(
        find.byKey(ValueKey('desktop-bot-${library.bots.single.id}')),
        findsNothing,
      );
      expect(find.byType(PressSurface), findsWidgets);
      expect(tester.takeException(), isNull);
    },
  );

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
      ModelCatalog.credentials = credentials;
      ModelCatalog.httpGet = (_, _) async =>
          const HttpResponse(200, '{"data":[{"id":"deepseek-chat"}]}');
      addTearDown(credentials.dispose);
      addTearDown(() {
        ModelCatalog.credentials = null;
        ModelCatalog.httpGet = null;
        ModelCatalog.invalidate();
      });

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
      expect(find.text('Connections & tools'), findsOneWidget);
      await tester.tap(find.text('Connections & tools').first);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 200));
      expect(find.text('Setup'), findsOneWidget);
      expect(find.text('Codex CLI'), findsOneWidget);
      expect(find.text('Claude Code'), findsOneWidget);
      expect(find.text('Grok CLI'), findsOneWidget);
      expect(find.text('API access'), findsOneWidget);
      await tester.drag(
        find.byKey(const ValueKey('cli-setup')),
        const Offset(0, -900),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 200));
      expect(find.text('DeepSeek API'), findsOneWidget);
      expect(find.text('Ollama Cloud API'), findsOneWidget);
      expect(find.text('Accounts'), findsNothing);
      expect(find.byType(SignInScreen), findsNothing);

      await tester.ensureVisible(find.text('Add').first);
      await tester.tap(find.text('Add').first);
      await pumpUiTransition(tester);
      await tester.enterText(
        find.byType(TextFormField),
        'deepseek-private-test',
      );
      await tester.tap(find.text('Save key'));
      await pumpUiTransition(tester);
      expect(credentials.forProvider(ProviderKind.deepseek)?.token, isNotEmpty);
      final reloadedCredentials = Credentials();
      await reloadedCredentials.load();
      expect(
        reloadedCredentials.forProvider(ProviderKind.deepseek)?.kind,
        CredentialKind.apiKey,
      );
      expect(
        reloadedCredentials.forProvider(ProviderKind.deepseek)?.token,
        isNotEmpty,
      );
      reloadedCredentials.dispose();
      await tester.ensureVisible(find.text('API key saved securely'));
      expect(find.text('API key saved securely'), findsOneWidget);
      expect(find.text('deepseek-private-test'), findsNothing);

      await tester.tap(find.text('Team').first);
      await pumpUiTransition(tester);
      await tester.tap(find.byTooltip('New bot'));
      await tester.pump(const Duration(milliseconds: 400));
      expect(find.textContaining('sign-in not verified'), findsNothing);
      expect(find.text('API key'), findsNothing);
      expect(find.textContaining('Sign in'), findsNothing);
      final desktopPicker = tester.widget<DropdownButton<ProviderKind>>(
        find.byType(DropdownButton<ProviderKind>).first,
      );
      expect(desktopPicker.items!.map((item) => item.value), [
        ProviderKind.codex,
        ProviderKind.anthropic,
        ProviderKind.grok,
        ProviderKind.deepseek,
        ProviderKind.ollama,
      ]);
      await tester.enterText(
        find.byWidgetPredicate(
          (widget) =>
              widget is TextField && widget.decoration?.labelText == 'Name',
        ),
        'DeepSeek teammate',
      );
      final providerField = find.byType(DropdownButtonFormField<ProviderKind>);
      await tester.ensureVisible(providerField);
      await tester.tap(providerField);
      await tester.pump(const Duration(milliseconds: 300));
      await tester.tap(find.text('DeepSeek API').last);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      await tester.tap(find.text('Save'));
      await pumpUiTransition(tester);
      final savedBot = library.bots.singleWhere(
        (bot) => bot.name == 'DeepSeek teammate',
      );
      expect(savedBot.provider, ProviderKind.deepseek);
      expect(savedBot.apiKey, isEmpty);

      await tester.tap(find.text('Connections & tools').first);
      await pumpUiTransition(tester);
      await tester.ensureVisible(find.byTooltip('Remove DeepSeek API key'));
      await tester.tap(find.byTooltip('Remove DeepSeek API key'));
      await pumpUiTransition(tester);
      expect(credentials.forProvider(ProviderKind.deepseek), isNull);

      await tester.tap(find.text('Team').first);
      await pumpUiTransition(tester);

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
      expect(
        library.bots.singleWhere((bot) => bot.name == 'Legacy').provider,
        ProviderKind.scripted,
      );

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
