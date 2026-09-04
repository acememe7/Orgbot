// Signing in on a device with no agent CLIs.
//
// Codex and Grok both publish a device-code flow, and both accept the client
// id their own CLI ships with — there is nothing to register. So a phone can
// sign in to a subscription on its own: it asks for a short code, the person
// types that code into a browser, and the provider hands back a token. The
// desktop's sign-ins are never involved.
//
// Anthropic is deliberately absent. It publishes no device flow, so a phone
// cannot sign in to a Claude subscription this way.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import '../models.dart';

/// What went wrong signing in.
class SignInError implements Exception {
  SignInError(this.message);

  /// Wording aimed at the person, not the log.
  final String message;

  @override
  String toString() => message;
}

/// The person took too long, or the code expired.
class SignInExpired extends SignInError {
  SignInExpired() : super('That code expired. Start again to get a new one.');
}

/// The person cancelled the flow.
class SignInCancelled extends SignInError {
  SignInCancelled() : super('Sign-in cancelled.');
}

/// A code to type in, and where to type it.
class DeviceChallenge {
  const DeviceChallenge({
    required this.userCode,
    required this.verificationUri,
    this.verificationUriComplete,
    this.interval = const Duration(seconds: 5),
  });

  /// The short code the person types into the browser.
  final String userCode;

  /// Where they type it.
  final String verificationUri;

  /// The same page with the code already filled in, when the provider offers
  /// one. Opening this is one tap instead of typing on a phone keyboard.
  final String? verificationUriComplete;

  /// How often the provider allows polling.
  final Duration interval;

  /// The best link to open: pre-filled if there is one.
  String get bestUri => verificationUriComplete ?? verificationUri;
}

/// A sign-in in progress: show the challenge, await the token.
class DeviceSignIn {
  DeviceSignIn(this.provider, this.challenge, this._token, this._cancel);

  /// Which backend is being signed in to.
  final ProviderKind provider;

  /// What to show the person right now.
  final DeviceChallenge challenge;

  final Future<OAuthToken> _token;
  final void Function() _cancel;

  /// Completes when the person approves, or fails with a [SignInError].
  Future<OAuthToken> get token => _token;

  /// Give up waiting.
  void cancel() => _cancel();
}

/// A token, and what is needed to renew it.
class OAuthToken {
  const OAuthToken({
    required this.accessToken,
    this.refreshToken,
    this.expiresAt,
    this.account,
    this.plan,
  });

  /// The secret. Never logged.
  final String accessToken;

  /// Used to get a new access token without asking the person again.
  final String? refreshToken;

  /// When [accessToken] stops working.
  final DateTime? expiresAt;

  /// The account it belongs to, for display.
  final String? account;

  /// The subscription or auth mode, for display.
  final String? plan;

  /// Redacts the secret: these reach logs and error messages.
  @override
  String toString() => 'OAuthToken(<redacted>, expires: $expiresAt)';
}

/// The Codex CLI's own public client. Registering an app is not required —
/// this is the id the CLI itself ships with.
const String kCodexClientId = 'app_EMoamEEZ73f0CkXaXp7hrann';
const String _codexDeviceStart =
    'https://auth.openai.com/api/accounts/deviceauth/usercode';
const String _codexDevicePoll =
    'https://auth.openai.com/api/accounts/deviceauth/token';
const String _codexTokenUrl = 'https://auth.openai.com/oauth/token';
const String _codexVerifyUrl = 'https://auth.openai.com/codex/device';
const String _codexRedirectUri =
    'https://auth.openai.com/deviceauth/callback';

/// The Grok CLI's own public OIDC client, likewise unregistered.
const String kGrokClientId = 'b1a00492-073a-47ea-816f-4c329264a828';
const String _grokDeviceStart = 'https://auth.x.ai/oauth2/device/code';
const String _grokTokenUrl = 'https://auth.x.ai/oauth2/token';
const String _grokScopes =
    'openid profile email offline_access grok-cli:access api:access';

/// Which providers a device with no CLIs can sign in to by itself.
const List<ProviderKind> kDeviceSignInProviders = [
  ProviderKind.codex,
  ProviderKind.grok,
];

/// Starts and completes device-code sign-ins.
///
/// The HTTP client is injectable so the flows can be tested without going out
/// to the network, and so the polling loop can be driven at speed.
class DeviceAuth {
  DeviceAuth({
    HttpTransport? transport,
    Duration? minInterval,
    Duration? maxInterval,
  })  : _http = transport ?? const _IoTransport(),
        _minInterval = minInterval ?? const Duration(seconds: 3),
        _maxInterval = maxInterval ?? const Duration(seconds: 10);

  final HttpTransport _http;

  /// A floor on the poll interval, so a provider asking for a fast poll
  /// cannot turn this into a busy loop.
  final Duration _minInterval;

  /// A ceiling, so a provider asking for a slow poll cannot leave someone
  /// staring at a spinner long after they have approved the code.
  final Duration _maxInterval;

  /// Begin signing in. Returns as soon as there is a code to show; the token
  /// arrives later on [DeviceSignIn.token].
  Future<DeviceSignIn> start(ProviderKind provider) async {
    switch (provider) {
      case ProviderKind.codex:
        return _startCodex();
      case ProviderKind.grok:
        return _startGrok();
      default:
        throw SignInError(
            '${provider.label} does not offer a sign-in this device can use.');
    }
  }

  // --- Codex -------------------------------------------------------------

  Future<DeviceSignIn> _startCodex() async {
    final started = await _http.postJson(
      _codexDeviceStart,
      {'client_id': kCodexClientId},
    );
    final deviceAuthId = _string(started, 'device_auth_id');
    final userCode = _string(started, 'user_code') ?? _string(started, 'usercode');
    if (deviceAuthId == null || userCode == null) {
      throw SignInError('ChatGPT did not return a sign-in code. Try again.');
    }
    final challenge = DeviceChallenge(
      userCode: userCode,
      verificationUri: _codexVerifyUrl,
      interval: _interval(started['interval']),
    );

    final cancelled = Completer<void>();
    final token = _pollCodex(deviceAuthId, userCode, challenge.interval, cancelled);
    return DeviceSignIn(ProviderKind.codex, challenge, token, () {
      if (!cancelled.isCompleted) cancelled.complete();
    });
  }

  Future<OAuthToken> _pollCodex(
    String deviceAuthId,
    String userCode,
    Duration interval,
    Completer<void> cancelled,
  ) async {
    final deadline = DateTime.now().add(const Duration(minutes: 15));
    while (DateTime.now().isBefore(deadline)) {
      if (cancelled.isCompleted) throw SignInCancelled();
      await Future<void>.delayed(interval);
      if (cancelled.isCompleted) throw SignInCancelled();

      final response = await _http.post(
        _codexDevicePoll,
        headers: const {
          'Accept': 'application/json',
          'Content-Type': 'application/json',
        },
        body: jsonEncode({
          'device_auth_id': deviceAuthId,
          'user_code': userCode,
        }),
      );
      // Not approved yet: the endpoint says so with a 403 or a 404 rather
      // than the usual authorization_pending body.
      if (response.status == 403 || response.status == 404) continue;
      if (response.status == 429) continue;
      if (response.status != 200) {
        throw SignInError('ChatGPT refused the sign-in (HTTP ' +
            response.status.toString() +
            ').');
      }
      final body = _decode(response.body);
      final code = _string(body, 'authorization_code');
      final verifier = _string(body, 'code_verifier');
      if (code == null || verifier == null) continue;
      return _exchangeCodex(code, verifier);
    }
    throw SignInExpired();
  }

  Future<OAuthToken> _exchangeCodex(String code, String verifier) async {
    final response = await _http.post(
      _codexTokenUrl,
      headers: const {
        'Accept': 'application/json',
        'Content-Type': 'application/x-www-form-urlencoded',
      },
      body: _form({
        'grant_type': 'authorization_code',
        'code': code,
        'redirect_uri': _codexRedirectUri,
        'client_id': kCodexClientId,
        'code_verifier': verifier,
      }),
    );
    if (response.status != 200) {
      throw SignInError('ChatGPT would not issue a token (HTTP ' +
          response.status.toString() +
          ').');
    }
    return _tokenFrom(_decode(response.body), plan: 'chatgpt');
  }

  // --- Grok --------------------------------------------------------------

  Future<DeviceSignIn> _startGrok() async {
    final response = await _http.post(
      _grokDeviceStart,
      headers: const {'Content-Type': 'application/x-www-form-urlencoded'},
      body: _form({'client_id': kGrokClientId, 'scope': _grokScopes}),
    );
    if (response.status != 200) {
      throw SignInError('Grok would not start a sign-in (HTTP ' +
          response.status.toString() +
          ').');
    }
    final started = _decode(response.body);
    final deviceCode = _string(started, 'device_code');
    final userCode = _string(started, 'user_code');
    final verify = _string(started, 'verification_uri');
    if (deviceCode == null || userCode == null || verify == null) {
      throw SignInError('Grok did not return a sign-in code. Try again.');
    }
    final challenge = DeviceChallenge(
      userCode: userCode,
      verificationUri: verify,
      verificationUriComplete: _string(started, 'verification_uri_complete'),
      interval: _interval(started['interval']),
    );

    final cancelled = Completer<void>();
    final expiresIn = started['expires_in'];
    final token = _pollGrok(
      deviceCode,
      challenge.interval,
      Duration(seconds: expiresIn is num ? expiresIn.toInt() : 900),
      cancelled,
    );
    return DeviceSignIn(ProviderKind.grok, challenge, token, () {
      if (!cancelled.isCompleted) cancelled.complete();
    });
  }

  Future<OAuthToken> _pollGrok(
    String deviceCode,
    Duration interval,
    Duration lifetime,
    Completer<void> cancelled,
  ) async {
    var wait = interval;
    final deadline = DateTime.now().add(lifetime);
    while (DateTime.now().isBefore(deadline)) {
      if (cancelled.isCompleted) throw SignInCancelled();
      await Future<void>.delayed(wait);
      if (cancelled.isCompleted) throw SignInCancelled();

      final response = await _http.post(
        _grokTokenUrl,
        headers: const {'Content-Type': 'application/x-www-form-urlencoded'},
        body: _form({
          'grant_type': 'urn:ietf:params:oauth:grant-type:device_code',
          'device_code': deviceCode,
          'client_id': kGrokClientId,
        }),
      );
      final body = _decode(response.body);
      if (response.status == 200) {
        return _tokenFrom(body, plan: 'grok');
      }
      // RFC 8628 names the states in the body rather than the status code.
      switch (_string(body, 'error')) {
        case 'authorization_pending':
          continue;
        case 'slow_down':
          wait += const Duration(seconds: 5);
          continue;
        case 'expired_token':
          throw SignInExpired();
        case 'access_denied':
          throw SignInError('That sign-in was declined.');
        default:
          throw SignInError('Grok refused the sign-in (HTTP ' +
              response.status.toString() +
              ').');
      }
    }
    throw SignInExpired();
  }

  // --- Renewal -----------------------------------------------------------

  /// Trade a refresh token for a fresh access token.
  ///
  /// Both providers expire access tokens within hours, so a device that was
  /// signed in yesterday still needs this before it can run a room.
  Future<OAuthToken> refresh(ProviderKind provider, String refreshToken) async {
    final url =
        provider == ProviderKind.grok ? _grokTokenUrl : _codexTokenUrl;
    final clientId =
        provider == ProviderKind.grok ? kGrokClientId : kCodexClientId;
    final response = await _http.post(
      url,
      headers: const {
        'Accept': 'application/json',
        'Content-Type': 'application/x-www-form-urlencoded',
      },
      body: _form({
        'grant_type': 'refresh_token',
        'refresh_token': refreshToken,
        'client_id': clientId,
      }),
    );
    if (response.status != 200) {
      throw SignInError('That sign-in has expired. Sign in again.');
    }
    final renewed = _tokenFrom(_decode(response.body), plan: null);
    // Some servers omit the refresh token when it has not rotated; keeping
    // the old one means the next renewal still works.
    return renewed.refreshToken == null
        ? OAuthToken(
            accessToken: renewed.accessToken,
            refreshToken: refreshToken,
            expiresAt: renewed.expiresAt,
            account: renewed.account,
            plan: renewed.plan,
          )
        : renewed;
  }

  // --- Shared ------------------------------------------------------------

  Duration _interval(Object? raw) {
    final seconds = raw is num
        ? raw.toInt()
        : raw is String
            ? int.tryParse(raw) ?? 5
            : 5;
    var wanted = Duration(seconds: seconds.clamp(1, 30));
    if (wanted > _maxInterval) wanted = _maxInterval;
    if (wanted < _minInterval) wanted = _minInterval;
    return wanted;
  }

  OAuthToken _tokenFrom(Map<String, Object?> body, {required String? plan}) {
    final access = _string(body, 'access_token');
    if (access == null) {
      throw SignInError('The sign-in came back without a token.');
    }
    final expiresIn = body['expires_in'];
    final claims = jwtClaims(access);
    final id = _string(body, 'id_token');
    final identity = id == null ? const <String, Object?>{} : jwtClaims(id);
    return OAuthToken(
      accessToken: access,
      refreshToken: _string(body, 'refresh_token'),
      expiresAt: expiresIn is num
          ? DateTime.now().add(Duration(seconds: expiresIn.toInt()))
          : null,
      account: _string(identity, 'email') ?? _string(claims, 'email'),
      plan: plan,
    );
  }
}

/// Decode a JWT's claims without verifying it.
///
/// Only ever used to read display fields and a routing id off a token this
/// device just received; the provider still authenticates the token itself,
/// so an unverified read is not an authorisation decision.
Map<String, Object?> jwtClaims(String token) {
  try {
    final parts = token.split('.');
    if (parts.length < 2) return const {};
    final payload = base64Url.decode(base64Url.normalize(parts[1]));
    final decoded = jsonDecode(utf8.decode(payload));
    return decoded is Map ? decoded.cast<String, Object?>() : const {};
  } catch (_) {
    return const {};
  }
}

/// The ChatGPT account a Codex token belongs to, which its backend wants as
/// its own header.
String? chatGptAccountId(String accessToken) {
  final claims = jwtClaims(accessToken);
  final auth = claims['https://api.openai.com/auth'];
  if (auth is Map) {
    final id = auth['chatgpt_account_id'];
    if (id is String && id.isNotEmpty) return id;
  }
  final flat = claims['chatgpt_account_id'];
  return flat is String && flat.isNotEmpty ? flat : null;
}

String? _string(Map<String, Object?> map, String key) {
  final value = map[key];
  return value is String && value.trim().isNotEmpty ? value : null;
}

Map<String, Object?> _decode(String body) {
  try {
    final decoded = jsonDecode(body);
    return decoded is Map ? decoded.cast<String, Object?>() : const {};
  } catch (_) {
    return const {};
  }
}

String _form(Map<String, String> fields) => fields.entries
    .map((e) =>
        Uri.encodeQueryComponent(e.key) + '=' + Uri.encodeQueryComponent(e.value))
    .join('&');

/// One HTTP response, reduced to what these flows need.
class HttpResponse {
  const HttpResponse(this.status, this.body);

  /// The status code.
  final int status;

  /// The body, as text.
  final String body;
}

/// The HTTP calls the flows make. Injectable so tests need no network.
abstract class HttpTransport {
  /// POST, returning the status and body rather than throwing on failure,
  /// because these flows treat several failure codes as "keep waiting".
  Future<HttpResponse> post(
    String url, {
    required Map<String, String> headers,
    required String body,
  });

  /// POST a JSON object and decode the reply.
  Future<Map<String, Object?>> postJson(String url, Map<String, Object?> body);
}

class _IoTransport implements HttpTransport {
  const _IoTransport();

  @override
  Future<HttpResponse> post(
    String url, {
    required Map<String, String> headers,
    required String body,
  }) async {
    final client = HttpClient()..connectionTimeout = const Duration(seconds: 20);
    try {
      final request = await client.postUrl(Uri.parse(url));
      headers.forEach(request.headers.set);
      request.write(body);
      final response = await request.close().timeout(const Duration(seconds: 30));
      final text = await response.transform(utf8.decoder).join();
      return HttpResponse(response.statusCode, text);
    } on SocketException {
      throw SignInError('No connection. Check the network and try again.');
    } on TimeoutException {
      throw SignInError('The provider did not answer in time. Try again.');
    } finally {
      client.close(force: true);
    }
  }

  @override
  Future<Map<String, Object?>> postJson(
      String url, Map<String, Object?> body) async {
    final response = await post(
      url,
      headers: const {
        'Accept': 'application/json',
        'Content-Type': 'application/json',
      },
      body: jsonEncode(body),
    );
    if (response.status == 429) {
      throw SignInError('Too many attempts just now. Wait a minute and retry.');
    }
    if (response.status != 200) {
      throw SignInError(
          'The provider refused the request (HTTP ' + response.status.toString() + ').');
    }
    return _decode(response.body);
  }
}
