// Where secrets are kept.
//
// Sign-ins used to sit in plaintext preferences. They now live in the
// platform keystore, and anything an older build left in the clear is moved
// across on first load and the plaintext copy removed — otherwise upgrading
// would leave the secret lying about forever.

import 'dart:convert';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _key = 'orgbot_credentials_v1';

String bundle(String token) => jsonEncode({
      'version': 1,
      'source': 'test',
      'providers': [
        {'provider': 'codex', 'token': token, 'kind': 'bearer'}
      ],
    });

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    FlutterSecureStorage.setMockInitialValues({});
  });

  test('a sign-in is written to the keystore, not to preferences', () async {
    final credentials = Credentials();
    await credentials.importBundle(bundle('secret-token'));

    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getString(_key), isNull,
        reason: 'nothing secret belongs in plaintext preferences');
    expect(await const FlutterSecureStorage().read(key: _key),
        contains('secret-token'));
  });

  test('what an older build left in the clear is moved and deleted', () async {
    SharedPreferences.setMockInitialValues({_key: bundle('legacy-token')});

    final credentials = Credentials();
    await credentials.load();

    expect(credentials.forProvider(ProviderKind.codex)?.token, 'legacy-token',
        reason: 'the existing sign-in must survive the upgrade');

    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getString(_key), isNull,
        reason: 'the plaintext copy must not be left behind');
    expect(await const FlutterSecureStorage().read(key: _key),
        contains('legacy-token'));
  });

  test('a token is not printed by toString', () {
    final credential = ProviderCredential(
      provider: ProviderKind.codex,
      token: 'do-not-log-me',
      kind: CredentialKind.bearer,
    );
    expect(credential.toString(), isNot(contains('do-not-log-me')));
    expect(credential.toString(), contains('redacted'));
  });

  // From the 13 November audit: a keyring locked at startup made the next
  // save write only the new sign-in, erasing the others.
  test('a keyring that was locked at startup is read again before a save',
      () async {
    await const FlutterSecureStorage().write(key: _key, value: bundle('kept'));
    final flaky = _LockedOnce();
    final credentials = Credentials(secure: flaky);
    await credentials.load();
    expect(credentials.all, isEmpty, reason: 'the first read failed');

    await credentials.saveApiKey(ProviderKind.deepseek, 'sk-new-key');

    final stored = await const FlutterSecureStorage().read(key: _key);
    expect(stored, contains('kept'), reason: 'the other sign-in survives');
    expect(stored, contains('sk-new-key'));
  });

  test('an empty store simply loads nothing', () async {
    final credentials = Credentials();
    await credentials.load();
    expect(credentials.all, isEmpty);
  });
}

/// A keystore whose first read fails, as a locked keyring does.
class _LockedOnce extends FlutterSecureStorage {
  _LockedOnce();

  var _reads = 0;

  @override
  Future<String?> read({
    required String key,
    AppleOptions? iOptions,
    AndroidOptions? aOptions,
    LinuxOptions? lOptions,
    WebOptions? webOptions,
    AppleOptions? mOptions,
    WindowsOptions? wOptions,
  }) {
    _reads++;
    if (_reads == 1) throw Exception('keyring locked');
    return const FlutterSecureStorage().read(key: key);
  }
}
