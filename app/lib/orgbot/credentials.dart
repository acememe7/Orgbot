// The sign-ins this device holds.
//
// A phone has no agent CLIs, so it signs in to a subscription itself with the
// device-code flow in `oauth/device_auth.dart`. Carrying a desktop's sign-ins
// across with `orgbot pair` still works and is the fallback for a provider
// with no device flow, but it is no longer the way in.
//
// Access tokens expire within hours, so what is stored is the refresh token
// alongside them; [Credentials.freshToken] renews on demand.
//
// Tokens are secrets. They are stored on the device and never written into a
// room file, and `toString` on anything here redacts them, because these
// objects otherwise end up in logs and error messages.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'model_catalog.dart';
import 'oauth/device_auth.dart';
import 'models.dart';

/// How a credential authenticates.
enum CredentialKind {
  /// An OAuth access token, sent as `Authorization: Bearer`.
  bearer,

  /// A long-lived API key.
  apiKey;

  static CredentialKind parse(String? raw) =>
      raw == 'api_key' ? CredentialKind.apiKey : CredentialKind.bearer;

  String get wire => this == CredentialKind.apiKey ? 'api_key' : 'bearer';
}

/// One provider's sign-in, plus the models it offers.
class ProviderCredential {
  ProviderCredential({
    required this.provider,
    required this.token,
    required this.kind,
    this.account,
    this.plan,
    this.models = const [],
    this.importedAt,
    this.refreshToken,
    this.expiresAt,
  });

  /// Build from a completed device-code sign-in.
  factory ProviderCredential.fromToken(
    ProviderKind provider,
    OAuthToken token, {
    List<ModelOption> models = const [],
  }) =>
      ProviderCredential(
        provider: provider,
        token: token.accessToken,
        kind: CredentialKind.bearer,
        account: token.account,
        plan: token.plan,
        models: models,
        importedAt: DateTime.now(),
        refreshToken: token.refreshToken,
        expiresAt: token.expiresAt,
      );

  /// Which backend this signs in to.
  final ProviderKind provider;

  /// The secret. Never logged, never written into a room file.
  final String token;

  final CredentialKind kind;

  /// The account it belongs to, for display.
  final String? account;

  /// The plan or auth mode, for display — "pro", "chatgpt", and so on.
  final String? plan;

  /// Models this provider offers, carried over from the desktop's own cache
  /// so a phone with no CLI still shows a real list rather than a guess.
  final List<ModelOption> models;

  /// When this was imported.
  final DateTime? importedAt;

  /// Renews [token] without asking the person again. Absent for a pasted
  /// bundle, whose token cannot be renewed on this device.
  final String? refreshToken;

  /// When [token] stops working, where the provider said.
  final DateTime? expiresAt;

  /// Whether the token is close enough to expiry to renew before using.
  ///
  /// The margin covers a room that starts just before the token lapses; a
  /// turn that begins valid must not fail partway through.
  bool get needsRenewal {
    final expiry = expiresAt;
    if (expiry == null) return false;
    return DateTime.now()
        .add(const Duration(minutes: 10))
        .isAfter(expiry);
  }

  /// Whether this can be renewed at all.
  bool get canRenew => (refreshToken ?? '').isNotEmpty;

  Map<String, Object?> toJson() => {
        'provider': provider.key,
        'token': token,
        'kind': kind.wire,
        'account': account,
        'plan': plan,
        'models': [
          for (final model in models) {'id': model.id, 'label': model.label},
        ],
        'imported_at': (importedAt ?? DateTime.now()).millisecondsSinceEpoch,
        'refresh_token': refreshToken,
        'expires_at': expiresAt?.millisecondsSinceEpoch,
      };

  static ProviderCredential? fromJson(Map<String, Object?> json) {
    final token = json['token'];
    if (token is! String || token.trim().isEmpty) return null;
    final provider = ProviderKind.fromKey(json['provider'] as String? ?? '');
    final rawModels = json['models'];
    return ProviderCredential(
      provider: provider,
      token: token,
      kind: CredentialKind.parse(json['kind'] as String?),
      account: json['account'] as String?,
      plan: json['plan'] as String?,
      models: rawModels is List
          ? rawModels
              .whereType<Map>()
              .map((m) => ModelOption(
                    (m['id'] ?? '').toString(),
                    (m['label'] ?? m['id'] ?? '').toString(),
                  ))
              .where((m) => m.id.isNotEmpty)
              .toList()
          : const [],
      importedAt: json['imported_at'] is num
          ? DateTime.fromMillisecondsSinceEpoch((json['imported_at'] as num).toInt())
          : null,
      refreshToken: json['refresh_token'] as String?,
      expiresAt: json['expires_at'] is num
          ? DateTime.fromMillisecondsSinceEpoch((json['expires_at'] as num).toInt())
          : null,
    );
  }

  /// Redacts the token: these objects reach logs and error messages.
  @override
  String toString() =>
      'ProviderCredential(${provider.key}, ${kind.wire}, <redacted>)';
}

/// What went wrong importing a bundle.
class PairingError implements Exception {
  PairingError(this.message);
  final String message;
  @override
  String toString() => message;
}

/// The bundle format this build understands.
const int kBundleVersion = 1;

/// The sign-ins held on this device.
class Credentials extends ChangeNotifier {
  // The field is filled lazily from SharedPreferences.getInstance() when it
  // was not injected, so it cannot be an initializing formal.
  // ignore: prefer_initializing_formals
  Credentials({SharedPreferences? prefs}) : _prefs = prefs;

  static const _key = 'orgbot_credentials_v1';

  SharedPreferences? _prefs;
  final Map<ProviderKind, ProviderCredential> _byProvider = {};

  /// Where the most recent bundle came from, for display.
  String? source;

  /// Every provider signed in here.
  List<ProviderCredential> get all => _byProvider.values.toList();

  /// The credential for a provider, if there is one.
  ProviderCredential? forProvider(ProviderKind provider) =>
      _byProvider[provider];

  /// Whether this provider can be used on this device.
  bool isSignedIn(ProviderKind provider) => _byProvider.containsKey(provider);

  /// Load what was stored previously.
  Future<void> load() async {
    final prefs = _prefs ??= await SharedPreferences.getInstance();
    final raw = prefs.getString(_key);
    if (raw == null || raw.isEmpty) return;
    try {
      final decoded = jsonDecode(raw);
      if (decoded is! Map) return;
      source = decoded['source'] as String?;
      final providers = decoded['providers'];
      if (providers is List) {
        for (final entry in providers.whereType<Map>()) {
          final credential =
              ProviderCredential.fromJson(entry.cast<String, Object?>());
          if (credential != null) _byProvider[credential.provider] = credential;
        }
      }
      notifyListeners();
    } catch (_) {
      // Corrupt storage should not stop the app starting; the person can
      // simply pair again.
    }
  }

  /// Import a bundle produced by `orgbot pair`.
  ///
  /// Accepts the raw JSON, with or without surrounding whitespace, so pasting
  /// from a terminal works without trimming by hand.
  Future<int> importBundle(String text) async {
    final trimmed = text.trim();
    if (trimmed.isEmpty) {
      throw PairingError('Nothing to import.');
    }
    Object? decoded;
    try {
      decoded = jsonDecode(trimmed);
    } catch (_) {
      throw PairingError(
          'That is not a pairing bundle. Run `orgbot pair` and paste all of '
          'its output.');
    }
    if (decoded is! Map) {
      throw PairingError('That is not a pairing bundle.');
    }
    final version = decoded['version'];
    if (version is num && version > kBundleVersion) {
      throw PairingError(
          'This bundle was made by a newer OrgBot (v$version). Update the app.');
    }
    final providers = decoded['providers'];
    if (providers is! List || providers.isEmpty) {
      throw PairingError('That bundle has no sign-ins in it.');
    }

    var imported = 0;
    for (final entry in providers.whereType<Map>()) {
      final credential =
          ProviderCredential.fromJson(entry.cast<String, Object?>());
      if (credential == null) continue;
      _byProvider[credential.provider] = credential;
      imported++;
    }
    if (imported == 0) {
      throw PairingError('No usable sign-ins were found in that bundle.');
    }
    source = decoded['source'] as String?;
    await _persist();
    notifyListeners();
    return imported;
  }

  /// Record a completed device-code sign-in.
  ///
  /// The models list is kept from any previous sign-in to the same provider,
  /// so signing in again after a token lapses does not empty the picker.
  Future<void> signIn(ProviderKind provider, OAuthToken token) async {
    final existing = _byProvider[provider];
    _byProvider[provider] = ProviderCredential.fromToken(
      provider,
      token,
      models: existing?.models ?? const [],
    );
    await _persist();
    notifyListeners();
  }

  /// Attach a freshly loaded model list to a sign-in.
  Future<void> setModels(
      ProviderKind provider, List<ModelOption> models) async {
    final existing = _byProvider[provider];
    if (existing == null || models.isEmpty) return;
    _byProvider[provider] = ProviderCredential(
      provider: existing.provider,
      token: existing.token,
      kind: existing.kind,
      account: existing.account,
      plan: existing.plan,
      models: models,
      importedAt: existing.importedAt,
      refreshToken: existing.refreshToken,
      expiresAt: existing.expiresAt,
    );
    await _persist();
    notifyListeners();
  }

  /// An access token that will still be valid when it is used, renewing first
  /// if the stored one is close to expiry.
  ///
  /// Returns null when this provider is not signed in, or when the sign-in has
  /// lapsed and cannot be renewed — the caller should send the person back to
  /// the sign-in screen rather than starting a room that will fail mid-turn.
  Future<String?> freshToken(ProviderKind provider, {DeviceAuth? auth}) async {
    final credential = _byProvider[provider];
    if (credential == null) return null;
    if (!credential.needsRenewal) return credential.token;
    if (!credential.canRenew) return null;
    try {
      final renewed = await (auth ?? DeviceAuth())
          .refresh(provider, credential.refreshToken!);
      await signIn(provider, renewed);
      return renewed.accessToken;
    } on SignInError {
      // A refusal here means the sign-in is gone for good; drop it so the UI
      // shows the provider as signed out rather than silently failing rooms.
      await forget(provider);
      return null;
    }
  }

  /// Forget one provider's sign-in.
  Future<void> forget(ProviderKind provider) async {
    _byProvider.remove(provider);
    await _persist();
    notifyListeners();
  }

  /// Forget everything.
  Future<void> forgetAll() async {
    _byProvider.clear();
    source = null;
    await _persist();
    notifyListeners();
  }

  Future<void> _persist() async {
    final prefs = _prefs ??= await SharedPreferences.getInstance();
    await prefs.setString(
      _key,
      jsonEncode({
        'version': kBundleVersion,
        'source': source,
        'providers': [for (final c in _byProvider.values) c.toJson()],
      }),
    );
  }
}
