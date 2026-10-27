import 'dart:io';

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
      await pumpUiTransition(tester);
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
      expect(find.text('Connections & tools'), findsNWidgets(2));
      expect(find.text('Codex CLI'), findsOneWidget);
      expect(find.text('Claude Code'), findsOneWidget);
      expect(find.text('Grok CLI'), findsOneWidget);
      if (Platform.isLinux) {
        expect(find.text('OpenCode CLI'), findsOneWidget);
        expect(find.text('Antigravity CLI'), findsOneWidget);
      }
      // Scroll to the rows rather than by a fixed distance, so sections
      // added further down the page do not carry them out of view.
      final setup = find
          .descendant(
            of: find.byKey(const ValueKey('cli-setup')),
            matching: find.byType(Scrollable),
          )
          .first;
      await tester.scrollUntilVisible(
        find.text('API access'),
        100,
        scrollable: setup,
      );
      await tester.pump(const Duration(milliseconds: 200));
      expect(find.text('API access'), findsOneWidget);
      for (final row in ['DeepSeek API', 'Ollama Cloud API']) {
        await tester.scrollUntilVisible(find.text(row), 100, scrollable: setup);
        await tester.pump(const Duration(milliseconds: 200));
        expect(find.text(row), findsOneWidget);
      }
      expect(find.text('Accounts'), findsNothing);
      expect(find.byType(SignInScreen), findsNothing);

      // Back to the DeepSeek row, so its Add button is the first one shown.
      await tester.scrollUntilVisible(
        find.text('DeepSeek API'),
        -100,
        scrollable: setup,
      );
      await tester.pump(const Duration(milliseconds: 200));
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
        if (Platform.isLinux) ...[
          ProviderKind.opencode,
          ProviderKind.antigravity,
        ],
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

      await tester.tap(
        find
            .ancestor(
              of: find.text('Connections & tools'),
              matching: find.byType(ListTile),
            )
            .first,
      );
      await pumpUiTransition(tester);
      final setupAfterEditor = find.byKey(const ValueKey('cli-setup'));
      final setupScroller = find
          .descendant(of: setupAfterEditor, matching: find.byType(Scrollable))
          .first;
      if (Platform.isLinux) {
        expect(find.byKey(const ValueKey('cli-setup')), findsOneWidget);
        await tester.drag(setupAfterEditor, const Offset(0, 1000));
        await tester.pump(const Duration(milliseconds: 200));
        expect(find.text('Antigravity CLI'), findsOneWidget);
        expect(
          find.text('Sign-in status has not been checked.'),
          findsOneWidget,
        );
        expect(find.text('Sign in'), findsOneWidget);
      }
      await tester.scrollUntilVisible(
        find.byTooltip('Remove DeepSeek API key'),
        100,
        scrollable: setupScroller,
      );
      await tester.ensureVisible(find.byTooltip('Remove DeepSeek API key'));
      await tester.drag(setupAfterEditor, const Offset(0, -12));
      await tester.pump(const Duration(milliseconds: 200));
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
      expect(
        androidPicker.items!.map((item) => item.value),
        isNot(contains(ProviderKind.opencode)),
      );
      expect(
        androidPicker.items!.map((item) => item.value),
        isNot(contains(ProviderKind.antigravity)),
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('Linux CLI models are explicit and keyless', (tester) async {
    if (!Platform.isLinux) return;
    SharedPreferences.setMockInitialValues({});
    FlutterSecureStorage.setMockInitialValues({});
    final library = Library(saver: MemorySaver());
    await library.load();
    addTearDown(library.dispose);
    final credentials = Credentials();
    await credentials.load();
    ModelCatalog.credentials = credentials;
    var queried = false;
    ModelCatalog.runner = (_, _) async {
      queried = true;
      throw StateError('the CLI model list must not be queried');
    };
    addTearDown(credentials.dispose);
    addTearDown(() {
      ModelCatalog.credentials = null;
      ModelCatalog.runner = null;
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
    await tester.tap(find.text('Team').first);
    await pumpUiTransition(tester);
    await tester.tap(find.byTooltip('New bot'));
    await tester.pump(const Duration(milliseconds: 400));

    for (final provider in [ProviderKind.opencode, ProviderKind.antigravity]) {
      final savedBeforeBlankAttempt = library.bots.length;
      final picker = find.byType(DropdownButtonFormField<ProviderKind>);
      await tester.ensureVisible(picker);
      await tester.pump();
      await tester.tap(picker);
      await tester.pump(const Duration(milliseconds: 200));
      await tester.tap(
        find.text(
          provider == ProviderKind.opencode
              ? 'OpenCode CLI'
              : 'Antigravity CLI',
        ),
      );
      await tester.pump(const Duration(milliseconds: 300));
      expect(find.text('CLI default'), findsNothing);
      expect(find.text('API key'), findsNothing);
      if (provider == ProviderKind.opencode) {
        expect(find.textContaining('sign-in not checked'), findsOneWidget);
        expect(
          find.text(
            'Uses the selected OpenCode API credential. OAuth reuse is unavailable.',
          ),
          findsOneWidget,
        );
      } else {
        expect(find.text('Execution unavailable'), findsOneWidget);
        expect(
          find.textContaining('Account access and protected execution'),
          findsOneWidget,
        );
      }
      expect(queried, isFalse);

      final modelField = find.byWidgetPredicate(
        (widget) =>
            widget is TextField && widget.decoration?.labelText == 'Model id',
      );
      await tester.ensureVisible(modelField);
      await tester.pump();
      expect(
        tester.widget<TextField>(modelField).decoration?.hintText,
        provider == ProviderKind.opencode ? 'provider/model' : 'model slug',
      );
      final nameField = find.byWidgetPredicate(
        (widget) =>
            widget is TextField && widget.decoration?.labelText == 'Name',
      );
      await tester.ensureVisible(nameField);
      await tester.enterText(nameField, provider.label);
      expect(tester.widget<TextField>(modelField).controller!.text, isEmpty);
      await tester.tap(find.text('Save'));
      await tester.pump();
      final botsAfterBlankSave = List<BotDef>.of(library.bots);
      expect(
        botsAfterBlankSave,
        hasLength(savedBeforeBlankAttempt),
        reason: botsAfterBlankSave
            .map((bot) => '${bot.provider.key}:${bot.model}')
            .join(', '),
      );
      expect(find.byType(SnackBar), findsOneWidget);
      await tester.pump(const Duration(seconds: 5));
      await tester.pump();
      await tester.ensureVisible(modelField);
      await tester.enterText(
        modelField,
        provider == ProviderKind.opencode
            ? 'test-provider/test-model'
            : 'test-model-slug',
      );
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await pumpUiTransition(tester);
      final saved = library.bots.singleWhere(
        (bot) => bot.name == provider.label,
      );
      expect(saved.provider, provider);
      expect(
        saved.model,
        provider == ProviderKind.opencode
            ? 'test-provider/test-model'
            : 'test-model-slug',
      );
      expect(saved.apiKey, isEmpty);

      if (provider == ProviderKind.opencode) {
        await tester.tap(find.byTooltip('New bot'));
        await tester.pump(const Duration(milliseconds: 400));
      }
    }
  });
}
