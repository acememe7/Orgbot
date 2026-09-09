import 'dart:convert';
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
import 'device_auth_test.dart' show FakeTransport, authWith;

const tokenUrl = 'https://auth.x.ai/oauth2/token';
String jwt(DateTime expiry) =>
    'h.${base64Url.encode(utf8.encode(jsonEncode({'exp': expiry.millisecondsSinceEpoch ~/ 1000}))).replaceAll('=', '')}.s';

void main() {
  setUp(() {
    FlutterSecureStorage.setMockInitialValues({});
    SharedPreferences.setMockInitialValues({});
    ModelCatalog.invalidate();
  });
  tearDown(() {
    ModelCatalog.credentials = null;
    ModelCatalog.deviceAuth = null;
    ModelCatalog.httpGet = null;
    ModelCatalog.invalidate();
  });

  Future<Credentials> signedIn({
    String token = 'saved',
    DateTime? expiry,
  }) async {
    final credentials = Credentials();
    await credentials.load();
    await credentials.signIn(
      ProviderKind.grok,
      OAuthToken(
        accessToken: token,
        refreshToken: 'refresh',
        expiresAt: expiry,
        plan: 'grok',
      ),
    );
    ModelCatalog.credentials = credentials;
    return credentials;
  }

  test(
    'legacy JWT expiry triggers one shared renewal and saves the rotated token',
    () async {
      final expired = jwt(DateTime.now().subtract(const Duration(hours: 1)));
      await signedIn(
        token: expired,
        expiry: DateTime.now().add(const Duration(hours: 6)),
      );
      final restored = Credentials();
      await restored.load();
      expect(restored.forProvider(ProviderKind.grok)!.needsRenewal, isTrue);
      final transport = FakeTransport({
        tokenUrl: [
          HttpResponse(
            200,
            jsonEncode({
              'access_token': 'renewed',
              'refresh_token': 'rotated',
              'expires_in': '600',
            }),
          ),
        ],
      });
      final auth = authWith(transport);
      final tokens = await Future.wait(
        List.generate(
          4,
          (_) => restored.freshToken(ProviderKind.grok, auth: auth),
        ),
      );
      expect(tokens, everyElement('renewed'));
      expect(transport.calls, hasLength(1));
      final reloaded = Credentials();
      await reloaded.load();
      expect(reloaded.forProvider(ProviderKind.grok)!.refreshToken, 'rotated');
      expect(reloaded.forProvider(ProviderKind.grok)!.needsRenewal, isFalse);
      final jwtExpiry = DateTime.now().add(const Duration(minutes: 10));
      final token = await authWith(
        FakeTransport({
          tokenUrl: [
            HttpResponse(200, jsonEncode({'access_token': jwt(jwtExpiry)})),
          ],
        }),
      ).refresh(ProviderKind.grok, 'refresh');
      expect(
        token.expiresAt!.difference(jwtExpiry).inSeconds.abs(),
        lessThan(2),
      );
    },
  );

  test(
    'a rejected access token renews once and loads subscription models',
    () async {
      final credentials = await signedIn(
        expiry: DateTime.now().add(const Duration(hours: 1)),
      );
      final transport = FakeTransport({
        tokenUrl: [
          const HttpResponse(
            200,
            '{"access_token":"renewed","expires_in":600}',
          ),
        ],
      });
      ModelCatalog.deviceAuth = authWith(transport);
      var calls = 0;
      ModelCatalog.httpGet = (url, headers) async {
        expect(url.toString(), 'https://cli-chat-proxy.grok.com/v1/models');
        expect(headers['X-XAI-Token-Auth'], 'xai-grok-cli');
        calls++;
        if (calls == 1) return const HttpResponse(401, '{}');
        expect(headers['authorization'], 'Bearer renewed');
        return const HttpResponse(
          200,
          '{"data":[{"id":"grok-4.6","name":"Grok 4.6"}]}',
        );
      };
      final list = await ModelCatalog.load(ProviderKind.grok, refresh: true);
      expect(list.source, ModelSource.live);
      expect(list.options.single.id, 'grok-4.6');
      expect(calls, 2);
      expect(transport.calls, hasLength(1));
      expect(credentials.isSignedIn(ProviderKind.grok), isTrue);
    },
  );

  test(
    '403 permissions and network failures keep the sign-in; revoked refresh does not',
    () async {
      final credentials = await signedIn();
      ModelCatalog.httpGet = (_, _) async =>
          const HttpResponse(403, '{"code":"permission-denied"}');
      final denied = await ModelCatalog.load(ProviderKind.grok);
      expect(denied.problem, contains('HTTP 403'));
      expect(denied.problem, isNot(contains('expired')));
      expect(credentials.isSignedIn(ProviderKind.grok), isTrue);
      ModelCatalog.httpGet = (_, _) async => const HttpResponse(401, '{}');
      ModelCatalog.deviceAuth = authWith(
        FakeTransport({
          tokenUrl: [const HttpResponse(503, '{}')],
        }),
      );
      final offline = await ModelCatalog.load(ProviderKind.grok);
      expect(offline.source, ModelSource.fallback);
      expect(credentials.isSignedIn(ProviderKind.grok), isTrue);
      ModelCatalog.deviceAuth = authWith(
        FakeTransport({
          tokenUrl: [const HttpResponse(400, '{"error":"invalid_grant"}')],
        }),
      );
      final revoked = await ModelCatalog.load(ProviderKind.grok);
      expect(revoked.source, ModelSource.needsSignIn);
      expect(revoked.options, isEmpty);
      expect(credentials.isSignedIn(ProviderKind.grok), isFalse);
    },
  );

  testWidgets('the editor never calls a failed catalog a verified sign-in', (
    tester,
  ) async {
    await signedIn();
    ModelCatalog.httpGet = (_, _) async => const HttpResponse(403, '{}');
    final library = Library(saver: MemorySaver());
    await library.load();
    library.addBot(
      BotDef(name: 'Teo', handle: 'teo', provider: ProviderKind.grok),
    );
    await tester.pumpWidget(MaterialApp(home: OrgBotScreen(library: library)));
    await tester.tap(find.text('Bots'));
    await tester.pumpAndSettle();
    await tester.tap(find.textContaining('@teo'));
    await tester.pumpAndSettle();
    expect(
      find.text('Sign-in saved · model access unavailable'),
      findsOneWidget,
    );
    expect(find.textContaining('Signed in —'), findsNothing);
    expect(find.textContaining('expired'), findsNothing);
    await tester.pumpWidget(const SizedBox.shrink());
    library.dispose();
  });
}
