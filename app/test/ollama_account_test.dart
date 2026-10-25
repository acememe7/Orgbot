import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:orgbot_app/main.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/model_catalog.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/oauth/device_auth.dart';
import 'package:orgbot_app/orgbot/room_file.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'package:orgbot_app/ui/sign_in_screen.dart';

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    FlutterSecureStorage.setMockInitialValues({});
    ModelCatalog.invalidate();
  });
  tearDown(() {
    ModelCatalog.credentials = null;
    ModelCatalog.httpGet = null;
    ModelCatalog.invalidate();
  });

  testWidgets(
    'Accounts saves Ollama separately and new bots reuse it after restart',
    (tester) async {
      final credentials = Credentials();
      await credentials.saveApiKey(ProviderKind.deepseek, 'deepseek-only');
      await tester.pumpWidget(
        MaterialApp(
          theme: orgBotTheme(Brightness.dark),
          home: SignInScreen(credentials: credentials),
        ),
      );
      await tester.tap(find.text('Add key'));
      await tester.pumpAndSettle();
      expect(find.text('Ollama Cloud API key'), findsOneWidget);
      expect(find.text('Get an Ollama API key'), findsOneWidget);
      final field = find.descendant(
        of: find.byType(AlertDialog),
        matching: find.byType(TextField),
      );
      expect(tester.widget<TextField>(field).obscureText, isTrue);
      await tester.enterText(field, '  ollama-only  ');
      await tester.tap(find.text('Save key'));
      await tester.pumpAndSettle();
      expect(find.text('Key saved for all your bots'), findsNWidgets(2));
      final restored = Credentials();
      await restored.load();
      expect(restored.forProvider(ProviderKind.ollama)?.token, 'ollama-only');
      expect(
        restored.forProvider(ProviderKind.deepseek)?.token,
        'deepseek-only',
      );
      expect(
        (await SharedPreferences.getInstance()).getString(
          'orgbot_credentials_v1',
        ),
        isNull,
      );
      expect(
        restored.forProvider(ProviderKind.ollama).toString(),
        isNot(contains('ollama-only')),
      );
      final bot = BotDef(
        name: 'Researcher',
        provider: ProviderKind.ollama,
        model: 'gpt-oss:120b',
      );
      expect(BotDef.fromJson(bot.toJson()).provider, ProviderKind.ollama);
      for (final cli in [false, true]) {
        final file = buildRoomFile(
          RoomDef(name: 'Cloud'),
          [bot],
          credentials: restored,
          canRunClis: cli,
        );
        expect(file.skipped, isEmpty);
        expect(file.toml, contains('kind = "openai"\nprofile = "ollama"'));
        expect(file.toml, contains('model = "gpt-oss:120b"'));
        expect(file.toml, isNot(contains('cli_agent')));
        expect(file.toml, isNot(contains('ollama-only')));
        expect(file.environment, {
          'ORGBOT_OLLAMA_API_KEY': 'ollama-only',
          'ORGBOT_OLLAMA_BASE_URL': 'https://ollama.com',
        });
      }
      await tester.pumpWidget(const SizedBox.shrink());
      credentials.dispose();
      restored.dispose();
    },
  );

  test(
    'catalog uses the correct account, reloads changed keys and explains failures',
    () async {
      final credentials = Credentials();
      ModelCatalog.credentials = credentials;
      expect(
        (await ModelCatalog.load(ProviderKind.ollama)).source,
        ModelSource.needsKey,
      );
      await credentials.saveApiKey(ProviderKind.ollama, 'ollama-only');
      var requests = 0;
      ModelCatalog.httpGet = (url, headers) async {
        requests++;
        expect(url.toString(), 'https://ollama.com/v1/models');
        expect(
          headers['authorization'],
          requests == 1 ? 'Bearer ollama-only' : 'Bearer replacement',
        );
        return HttpResponse(
          200,
          jsonEncode({
            'data': [
              {'id': 'gpt-oss:120b'},
              {'id': ''},
              {'id': 'gpt-oss:120b'},
            ],
          }),
        );
      };
      final list = await ModelCatalog.load(ProviderKind.ollama);
      expect(list.source, ModelSource.live);
      expect(list.options.map((m) => m.id), ['gpt-oss:120b']);
      await ModelCatalog.load(ProviderKind.ollama, apiKey: 'replacement');
      expect(
        requests,
        2,
        reason: 'A previous account catalog must not hide a changed key',
      );
      for (final status in [401, 403, 429, 500]) {
        ModelCatalog.httpGet = (_, _) async => HttpResponse(status, '{}');
        final failed = await ModelCatalog.load(ProviderKind.ollama);
        expect(failed.options, isEmpty);
        expect(
          failed.source,
          status == 401 ? ModelSource.needsKey : ModelSource.fallback,
        );
        expect(failed.problem, isNot(contains('Instance of')));
        expect(credentials.isSignedIn(ProviderKind.ollama), isTrue);
      }
      ModelCatalog.httpGet = (_, _) async => HttpResponse(200, '{}');
      expect(
        (await ModelCatalog.load(ProviderKind.ollama)).source,
        ModelSource.fallback,
      );
      ModelCatalog.httpGet = (url, headers) async {
        expect(url.host, 'api.deepseek.com');
        expect(headers['authorization'], 'Bearer deepseek-only');
        return HttpResponse(200, '{"data":[{"id":"deepseek-chat"}]}');
      };
      expect(
        (await ModelCatalog.load(
          ProviderKind.deepseek,
          apiKey: 'deepseek-only',
        )).options.single.id,
        'deepseek-chat',
      );
      credentials.dispose();
    },
  );

  test(
    'missing setup is skipped and distinct Ollama bot keys stay isolated',
    () {
      final file = buildRoomFile(RoomDef(name: 'Cloud'), [
        BotDef(
          name: 'No key',
          provider: ProviderKind.ollama,
          model: 'gpt-oss:120b',
        ),
        BotDef(
          name: 'No model',
          provider: ProviderKind.ollama,
          apiKey: 'unused',
        ),
        BotDef(
          name: 'one',
          provider: ProviderKind.ollama,
          apiKey: 'key-one',
          model: 'gpt-oss:120b',
        ),
        BotDef(
          name: 'two',
          provider: ProviderKind.ollama,
          apiKey: 'key-two',
          model: 'gpt-oss:20b',
        ),
      ], canRunClis: false);
      expect(file.skipped.length, 2);
      expect(file.environment['ORGBOT_OLLAMA_API_KEY'], 'key-one');
      expect(file.environment['ORGBOT_OLLAMA_TWO_API_KEY'], 'key-two');
      expect(
        file.environment['ORGBOT_OLLAMA_TWO_BASE_URL'],
        'https://ollama.com',
      );
      expect(file.toml, contains('profile = "ollama_two"'));
      expect(file.toml, isNot(contains('kind = "scripted"')));
    },
  );

  testWidgets(
    'switching the bot provider clears an entered key before model lookup',
    (tester) async {
      final library = Library(saver: MemorySaver());
      await library.load();
      library.addBot(
        BotDef(
          name: 'Researcher',
          provider: ProviderKind.deepseek,
          apiKey: 'deepseek-only',
          model: 'deepseek-chat',
        ),
      );
      final hosts = <String>[];
      ModelCatalog.httpGet = (url, headers) async {
        hosts.add(url.host);
        expect(url.host, 'api.deepseek.com');
        return HttpResponse(200, '{"data":[{"id":"deepseek-chat"}]}');
      };
      await tester.pumpWidget(
        MaterialApp(
          home: OrgBotScreen(library: library, desktopClis: false),
        ),
      );
      await tester.tap(find.text('Bots'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Researcher'));
      await tester.pumpAndSettle();
      final dropdown = find.byType(DropdownButtonFormField<ProviderKind>);
      await tester.ensureVisible(dropdown);
      await tester.tap(dropdown);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Ollama Cloud').last);
      await tester.pumpAndSettle();
      final field = find.byWidgetPredicate(
        (w) => w is TextField && w.obscureText,
      );
      expect(tester.widget<TextField>(field).controller!.text, isEmpty);
      expect(hosts, ['api.deepseek.com']);
      expect(
        find.textContaining('Ollama Cloud API key in Accounts'),
        findsOneWidget,
      );
      await tester.pumpWidget(const SizedBox.shrink());
      library.dispose();
    },
  );
}
