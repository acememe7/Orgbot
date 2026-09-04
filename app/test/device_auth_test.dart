// The device-code sign-ins, driven without a network.
//
// These cover the shapes the two providers actually return — which differ:
// Codex answers "not yet" with an HTTP status, Grok with an RFC 8628 error
// body — and the failure paths a person will hit on a phone.

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/oauth/device_auth.dart';

/// A scripted transport: each URL yields its queued responses in order.
class FakeTransport implements HttpTransport {
  FakeTransport(this.responses);

  /// Queued responses per URL. The last one repeats once exhausted, so a
  /// polling loop does not fall off the end of the script.
  final Map<String, List<HttpResponse>> responses;

  /// Every request made, for asserting on the wire format.
  final List<({String url, String body})> calls = [];

  @override
  Future<HttpResponse> post(
    String url, {
    required Map<String, String> headers,
    required String body,
  }) async {
    calls.add((url: url, body: body));
    final queued = responses[url];
    if (queued == null || queued.isEmpty) {
      return const HttpResponse(500, '{}');
    }
    return queued.length == 1 ? queued.first : queued.removeAt(0);
  }

  @override
  Future<Map<String, Object?>> postJson(
      String url, Map<String, Object?> body) async {
    final response = await post(
      url,
      headers: const {},
      body: jsonEncode(body),
    );
    if (response.status != 200) {
      throw SignInError('HTTP ${response.status}');
    }
    final decoded = jsonDecode(response.body);
    return decoded is Map ? decoded.cast<String, Object?>() : {};
  }
}

const _codexStart =
    'https://auth.openai.com/api/accounts/deviceauth/usercode';
const _codexPoll = 'https://auth.openai.com/api/accounts/deviceauth/token';
const _codexToken = 'https://auth.openai.com/oauth/token';
const _grokStart = 'https://auth.x.ai/oauth2/device/code';
const _grokToken = 'https://auth.x.ai/oauth2/token';

/// A JWT whose claims carry an account id and an email.
/// {"email":"a@b.c","https://api.openai.com/auth":{"chatgpt_account_id":"acct_7"}}
const _jwt = 'h.'
    'eyJlbWFpbCI6ImFAYi5jIiwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfYWNjb3VudF9pZCI6ImFjY3RfNyJ9fQ'
    '.s';

DeviceAuth authWith(FakeTransport transport) => DeviceAuth(
      transport: transport,
      // Poll immediately; the flows are what is under test, not the waiting.
      minInterval: Duration.zero,
      maxInterval: Duration.zero,
    );

void main() {
  group('Codex', () {
    test('shows a code, then exchanges the approval for a token', () async {
      final transport = FakeTransport({
        _codexStart: [
          HttpResponse(200,
              jsonEncode({'device_auth_id': 'da_1', 'user_code': 'UWON-C8ZXH', 'interval': '5'})),
        ],
        _codexPoll: [
          // Not approved yet: this endpoint says so with a status, not a body.
          const HttpResponse(403, '{}'),
          HttpResponse(200,
              jsonEncode({'authorization_code': 'ac_1', 'code_verifier': 'cv_1'})),
        ],
        _codexToken: [
          HttpResponse(200,
              jsonEncode({'access_token': _jwt, 'refresh_token': 'rt_1', 'expires_in': 3600})),
        ],
      });

      final flow = await authWith(transport).start(ProviderKind.codex);
      expect(flow.challenge.userCode, 'UWON-C8ZXH');
      expect(flow.challenge.verificationUri,
          'https://auth.openai.com/codex/device');

      final token = await flow.token;
      expect(token.accessToken, _jwt);
      expect(token.refreshToken, 'rt_1');
      expect(token.account, 'a@b.c');
      expect(token.expiresAt, isNotNull);

      // The exchange must carry the verifier the poll handed back.
      final exchange = transport.calls.last;
      expect(exchange.url, _codexToken);
      expect(exchange.body, contains('code_verifier=cv_1'));
      expect(exchange.body, contains('client_id=' + kCodexClientId));
    });

    test('a start with no code in it is reported, not left hanging', () async {
      final transport = FakeTransport({
        _codexStart: [const HttpResponse(200, '{}')],
      });
      expect(
        () => authWith(transport).start(ProviderKind.codex),
        throwsA(isA<SignInError>()),
      );
    });
  });

  group('Grok', () {
    test('polls past authorization_pending and returns the token', () async {
      final transport = FakeTransport({
        _grokStart: [
          HttpResponse(
              200,
              jsonEncode({
                'device_code': 'dc_1',
                'user_code': '9NQC-2Y9T',
                'verification_uri': 'https://accounts.x.ai/oauth2/device',
                'verification_uri_complete':
                    'https://accounts.x.ai/oauth2/device?user_code=9NQC-2Y9T',
                'expires_in': 1800,
                'interval': 5,
              })),
        ],
        _grokToken: [
          HttpResponse(400, jsonEncode({'error': 'authorization_pending'})),
          HttpResponse(200,
              jsonEncode({'access_token': 'grok_at', 'refresh_token': 'grok_rt', 'expires_in': 21600})),
        ],
      });

      final flow = await authWith(transport).start(ProviderKind.grok);
      expect(flow.challenge.userCode, '9NQC-2Y9T');
      // The pre-filled link is one tap rather than typing on a phone.
      expect(flow.challenge.bestUri, contains('user_code=9NQC-2Y9T'));

      final token = await flow.token;
      expect(token.accessToken, 'grok_at');
      expect(token.refreshToken, 'grok_rt');

      final poll = transport.calls.last;
      expect(poll.body,
          contains('grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code'));
      expect(poll.body, contains('device_code=dc_1'));
    });

    test('an expired code says so rather than waiting forever', () async {
      final transport = FakeTransport({
        _grokStart: [
          HttpResponse(
              200,
              jsonEncode({
                'device_code': 'dc_1',
                'user_code': 'X',
                'verification_uri': 'https://accounts.x.ai/oauth2/device',
                'interval': 1,
              })),
        ],
        _grokToken: [
          HttpResponse(400, jsonEncode({'error': 'expired_token'})),
        ],
      });
      final flow = await authWith(transport).start(ProviderKind.grok);
      await expectLater(flow.token, throwsA(isA<SignInExpired>()));
    });

    test('a declined approval is reported', () async {
      final transport = FakeTransport({
        _grokStart: [
          HttpResponse(
              200,
              jsonEncode({
                'device_code': 'dc_1',
                'user_code': 'X',
                'verification_uri': 'https://accounts.x.ai/oauth2/device',
                'interval': 1,
              })),
        ],
        _grokToken: [
          HttpResponse(400, jsonEncode({'error': 'access_denied'})),
        ],
      });
      final flow = await authWith(transport).start(ProviderKind.grok);
      await expectLater(flow.token, throwsA(isA<SignInError>()));
    });

    test('cancelling stops the poll', () async {
      final transport = FakeTransport({
        _grokStart: [
          HttpResponse(
              200,
              jsonEncode({
                'device_code': 'dc_1',
                'user_code': 'X',
                'verification_uri': 'https://accounts.x.ai/oauth2/device',
                'interval': 1,
              })),
        ],
        _grokToken: [
          HttpResponse(400, jsonEncode({'error': 'authorization_pending'})),
        ],
      });
      final flow = await authWith(transport).start(ProviderKind.grok);
      flow.cancel();
      await expectLater(flow.token, throwsA(isA<SignInCancelled>()));
    });
  });

  group('renewal', () {
    test('a refresh keeps the old refresh token when none comes back',
        () async {
      final transport = FakeTransport({
        _grokToken: [
          HttpResponse(200,
              jsonEncode({'access_token': 'new_at', 'expires_in': 21600})),
        ],
      });
      final token =
          await authWith(transport).refresh(ProviderKind.grok, 'old_rt');
      expect(token.accessToken, 'new_at');
      // Servers omit it when it has not rotated; dropping it would strand the
      // next renewal.
      expect(token.refreshToken, 'old_rt');
    });

    test('a rejected refresh asks the person to sign in again', () async {
      final transport = FakeTransport({
        _grokToken: [const HttpResponse(400, '{"error":"invalid_grant"}')],
      });
      expect(
        () => authWith(transport).refresh(ProviderKind.grok, 'stale'),
        throwsA(isA<SignInError>()),
      );
    });
  });

  group('token claims', () {
    test('the ChatGPT account id is read from the claims', () {
      expect(chatGptAccountId(_jwt), 'acct_7');
    });

    test('a token that is not a JWT yields nothing rather than throwing', () {
      expect(chatGptAccountId('not-a-jwt'), isNull);
      expect(jwtClaims('a.!!!.c'), isEmpty);
    });
  });

  test('a provider with no device flow is refused up front', () {
    expect(
      () => authWith(FakeTransport({})).start(ProviderKind.anthropic),
      throwsA(isA<SignInError>()),
    );
  });
}
